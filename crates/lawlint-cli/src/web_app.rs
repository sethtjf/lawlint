//! Local browser workspace for the primary interactive lawlint experience.
//!
//! The UI is deliberately embedded and the HTTP server is deliberately small:
//! a release binary needs no Node runtime, async executor, desktop webview, or
//! certificate. It binds to loopback only and every API request carries the
//! random token from the launch URL.

use crate::{ai_decision, build_rule_set, find_config, lint_text, merge_options, AiOff};
use lawlint_core::{apply_fixes, LintOptions, LintResult, RuleSet};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const INDEX_HTML: &str = include_str!("web/index.html");
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    config: LintOptions,
    rules: RuleSet,
    initial: Option<InitialDocument>,
    token: String,
    origin: String,
    shutdown: Arc<AtomicBool>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitialDocument {
    source_name: String,
    text: String,
    markdown: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LintRequest {
    text: String,
    #[serde(default)]
    markdown: bool,
    #[serde(default)]
    no_ai: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LintResponse {
    result: LintResult,
    /// The plain-text machine-applicable result, used by the download button.
    fixed_text: String,
    /// The extracted text is returned for a DOCX upload so the editor can show
    /// the same text the native engine reviewed.
    text: String,
    source_name: String,
    ai_status: String,
}

struct Response {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
    extra_headers: Vec<(String, String)>,
}

struct Request {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

pub fn run(initial_path: Option<PathBuf>) -> Result<i32, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    let (config, config_dir) = find_config(cwd.clone())?;
    let rules = build_rule_set(&config, config_dir.as_deref(), &[])?;
    let initial = initial_path.map(|path| load_document(&path)).transpose()?;

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|error| format!("could not start local workspace: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("could not configure local workspace: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("could not determine workspace port: {error}"))?
        .port();
    let token = session_token();
    let origin = format!("http://127.0.0.1:{port}");
    let url = format!("{origin}/?token={token}");
    let state = Arc::new(AppState {
        config,
        rules,
        initial,
        token,
        origin,
        shutdown: Arc::new(AtomicBool::new(false)),
    });

    println!("lawlint workspace: {url}");
    println!(
        "Keep this command running while you use the browser workspace. Press Ctrl-C to stop."
    );
    open_browser(&url);

    while !state.shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                std::thread::spawn(move || handle_connection(stream, state));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(error) => return Err(format!("local workspace stopped: {error}")),
        }
    }
    Ok(0)
}

fn handle_connection(mut stream: TcpStream, state: Arc<AppState>) {
    let response = match read_request(&mut stream) {
        Ok(request) => route(request, &state),
        Err(error) => error_response(400, &error),
    };
    let _ = write_response(&mut stream, response);
}

fn route(request: Request, state: &AppState) -> Response {
    let (path, query) = request
        .target
        .split_once('?')
        .map_or((request.target.as_str(), ""), |(path, query)| (path, query));

    if !origin_allowed(&request, state) {
        return error_response(
            403,
            "This workspace only accepts requests from its local page.",
        );
    }

    if path == "/" || path == "/index.html" {
        if query_token(query) != Some(state.token.as_str()) {
            return error_response(401, "This workspace link has expired. Run lawlint again.");
        }
        return html_response(INDEX_HTML);
    }

    if !authorized(&request, state) {
        return error_response(401, "Missing or invalid workspace token.");
    }

    match (request.method.as_str(), path) {
        ("GET", "/api/session") => session_response(state),
        ("POST", "/api/lint") => json_lint_response(state, &request.body),
        ("POST", "/api/lint-file") => file_lint_response(state, &request),
        ("POST", "/api/fix-file") => file_fix_response(state, &request),
        ("POST", "/api/shutdown") => {
            state.shutdown.store(true, Ordering::Relaxed);
            json_response(200, &serde_json::json!({"ok": true}))
        }
        _ => error_response(404, "Not found."),
    }
}

fn session_response(state: &AppState) -> Response {
    let ai_status = match ai_decision(&None, false, &state.config) {
        Ok(Ok(_)) => "AI review ready".to_string(),
        Ok(Err(reason)) => format!("AI review {}", reason.reason()),
        Err(error) => format!("AI review unavailable: {error}"),
    };
    let initial = state.initial.as_ref();
    json_response(
        200,
        &serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "ruleCount": state.rules.metas().len(),
            "aiStatus": ai_status,
            "initialText": initial.map(|document| document.text.as_str()).unwrap_or(""),
            "sourceName": initial.map(|document| document.source_name.as_str()).unwrap_or("Untitled document"),
            "markdown": initial.map(|document| document.markdown).unwrap_or(false),
        }),
    )
}

fn json_lint_response(state: &AppState, body: &[u8]) -> Response {
    let request: LintRequest = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => return error_response(400, &format!("Invalid lint request: {error}")),
    };
    match lint_document(
        state,
        request.text,
        request.markdown,
        request.no_ai,
        "Untitled document".to_string(),
    ) {
        Ok(response) => json_response(200, &response),
        Err(error) => error_response(400, &error),
    }
}

fn file_lint_response(state: &AppState, request: &Request) -> Response {
    let filename = request_filename(request);
    let no_ai = request
        .headers
        .get("x-lawlint-no-ai")
        .is_some_and(|value| value == "1");
    match decode_document(&filename, &request.body) {
        Ok((text, markdown)) => match lint_document(state, text, markdown, no_ai, filename) {
            Ok(response) => json_response(200, &response),
            Err(error) => error_response(400, &error),
        },
        Err(error) => error_response(400, &error),
    }
}

fn file_fix_response(state: &AppState, request: &Request) -> Response {
    let filename = request_filename(request);
    let is_docx = is_docx(&filename);
    let (text, markdown) = match decode_document(&filename, &request.body) {
        Ok(document) => document,
        Err(error) => return error_response(400, &error),
    };
    let no_ai = request
        .headers
        .get("x-lawlint-no-ai")
        .is_some_and(|value| value == "1");
    let lint = match lint_document(state, text.clone(), markdown, no_ai, filename.clone()) {
        Ok(response) => response,
        Err(error) => return error_response(400, &error),
    };
    if is_docx {
        let revised = match lawlint_docx::apply_tracked_changes(
            &request.body,
            &lint.result.diagnostics,
            &lawlint_docx::ReviseOptions::default(),
        ) {
            Ok(result) => result.bytes,
            Err(error) => {
                return error_response(400, &format!("Could not create Word review: {error}"))
            }
        };
        return binary_response(
            200,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            revised,
            format!("lawlint-fixed-{}", safe_filename(&filename)),
        );
    }
    binary_response(
        200,
        "text/plain; charset=utf-8",
        lint.fixed_text.into_bytes(),
        format!("lawlint-fixed-{}.txt", safe_stem(&filename)),
    )
}

fn lint_document(
    state: &AppState,
    text: String,
    markdown: bool,
    no_ai: bool,
    source_name: String,
) -> Result<LintResponse, String> {
    let decision = match ai_decision(&None, no_ai, &state.config) {
        Ok(decision) => decision,
        Err(error) => {
            eprintln!("lawlint: browser AI review unavailable ({error}); using static review");
            Err(AiOff::NoModel)
        }
    };
    let ai_status = match &decision {
        Ok(_) => "AI review included".to_string(),
        Err(reason) => format!("AI review skipped: {}", reason.reason()),
    };
    let judge = decision.as_ref().ok().cloned();
    let options = merge_options(
        state.config.clone(),
        LintOptions {
            markdown: Some(markdown),
            ..LintOptions::default()
        },
    );
    let result = lint_text(&text, &options, &state.rules, judge);
    let fixed_text = apply_fixes(&text, &result.diagnostics);
    Ok(LintResponse {
        result,
        fixed_text,
        text,
        source_name,
        ai_status,
    })
}

fn load_document(path: &Path) -> Result<InitialDocument, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let (text, markdown) = decode_document(&path.display().to_string(), &bytes)?;
    Ok(InitialDocument {
        source_name: path.display().to_string(),
        text,
        markdown,
    })
}

fn decode_document(filename: &str, bytes: &[u8]) -> Result<(String, bool), String> {
    if is_docx(filename) {
        let text = lawlint_docx::extract(bytes)
            .map_err(|error| format!("failed to read {filename}: {error}"))?;
        return Ok((text, false));
    }
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|_| format!("{filename} is not UTF-8 text; open a .docx, .md, or .txt file"))?;
    Ok((
        text,
        Path::new(filename)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
    ))
}

fn request_filename(request: &Request) -> String {
    request.headers.get("x-lawlint-filename").map_or_else(
        || "document.txt".to_string(),
        |value| percent_decode(value).unwrap_or_else(|| value.clone()),
    )
}

fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|byte| hex_value(*byte))?;
            let low = bytes.get(index + 2).and_then(|byte| hex_value(*byte))?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_docx(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("docx"))
}

fn read_request(stream: &mut TcpStream) -> Result<Request, String> {
    let mut buffer = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("connection closed before the request was complete".into());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_BODY_BYTES + 64 * 1024 {
            return Err("request is too large".into());
        }
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header_text =
        std::str::from_utf8(&buffer[..header_end]).map_err(|_| "invalid HTTP headers")?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().ok_or("missing HTTP request line")?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or("missing HTTP method")?.to_string();
    let target = parts.next().ok_or("missing HTTP target")?.to_string();
    let mut headers = HashMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').ok_or("invalid HTTP header")?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>().map_err(|_| "invalid Content-Length"))
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err("request body is too large".into());
    }
    if headers
        .get("expect")
        .is_some_and(|value| value.eq_ignore_ascii_case("100-continue"))
    {
        stream
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .map_err(|error| error.to_string())?;
    }
    let mut body = buffer[header_end..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0_u8; 8192];
        let read = stream.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("connection closed before the request body was complete".into());
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Request {
        method,
        target,
        headers,
        body,
    })
}

fn write_response(stream: &mut TcpStream, response: Response) -> Result<(), String> {
    let mut head = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    for (name, value) in response.extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .map_err(|error| error.to_string())?;
    stream
        .write_all(&response.body)
        .map_err(|error| error.to_string())
}

fn html_response(html: &str) -> Response {
    Response {
        status: "200 OK",
        content_type: "text/html; charset=utf-8",
        body: html.as_bytes().to_vec(),
        extra_headers: vec![
            (
                "Content-Security-Policy".into(),
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; form-action 'none'".into(),
            ),
            ("X-Content-Type-Options".into(), "nosniff".into()),
        ],
    }
}

fn json_response<T: Serialize>(status: u16, value: &T) -> Response {
    let body = serde_json::to_vec(value)
        .unwrap_or_else(|_| b"{\"error\":\"serialization failure\"}".to_vec());
    Response {
        status: status_text(status),
        content_type: "application/json; charset=utf-8",
        body,
        extra_headers: Vec::new(),
    }
}

fn error_response(status: u16, message: &str) -> Response {
    json_response(status, &serde_json::json!({ "error": message }))
}

fn binary_response(
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    filename: String,
) -> Response {
    Response {
        status: status_text(status),
        content_type,
        body,
        extra_headers: vec![(
            "Content-Disposition".into(),
            format!("attachment; filename=\"{filename}\""),
        )],
    }
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "200 OK",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        413 => "413 Payload Too Large",
        _ => "500 Internal Server Error",
    }
}

fn authorized(request: &Request, state: &AppState) -> bool {
    request
        .headers
        .get("x-lawlint-token")
        .is_some_and(|token| token == &state.token)
}

fn origin_allowed(request: &Request, state: &AppState) -> bool {
    request
        .headers
        .get("origin")
        .is_none_or(|origin| origin == &state.origin)
}

fn query_token(query: &str) -> Option<&str> {
    query
        .split('&')
        .find_map(|part| part.strip_prefix("token="))
}

fn session_token() -> String {
    let mut bytes = [0_u8; 24];
    if let Ok(mut file) = fs::File::open("/dev/urandom") {
        if file.read_exact(&mut bytes).is_ok() {
            return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        }
    }
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ u128::from(std::process::id());
    format!("{seed:032x}")
}

fn safe_filename(filename: &str) -> String {
    Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document.docx")
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || matches!(char, '.' | '-' | '_') {
                char
            } else {
                '_'
            }
        })
        .collect()
}

fn safe_stem(filename: &str) -> String {
    let filename = safe_filename(filename);
    filename
        .rsplit_once('.')
        .map_or(filename.as_str(), |(stem, _)| stem)
        .to_string()
}

fn open_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(url).spawn();
    if result.is_err() {
        eprintln!("lawlint: could not open your browser automatically; copy the URL above.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        AppState {
            config: LintOptions::default(),
            rules: RuleSet::built_in(),
            initial: None,
            token: "test-token".into(),
            origin: "http://127.0.0.1:12345".into(),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn token_is_unguessable_shaped() {
        assert_eq!(session_token().len(), 48);
    }

    #[test]
    fn filenames_cannot_escape_download_name() {
        assert_eq!(safe_filename("../../draft.docx"), "draft.docx");
        assert_eq!(safe_filename("draft\".docx"), "draft_.docx");
        assert_eq!(safe_stem("draft.md"), "draft");
    }

    #[test]
    fn percent_encoded_filenames_decode_for_browser_uploads() {
        assert_eq!(
            percent_decode("r%C3%A9sum%C3%A9%20draft.docx").as_deref(),
            Some("résumé draft.docx")
        );
        assert_eq!(percent_decode("draft%ZZ.docx"), None);
    }

    #[test]
    fn api_requires_the_launch_token() {
        let state = test_state();
        let request = Request {
            method: "GET".into(),
            target: "/api/session".into(),
            headers: HashMap::new(),
            body: Vec::new(),
        };
        assert_eq!(route(request, &state).status, "401 Unauthorized");
    }

    #[test]
    fn lint_api_uses_the_native_rule_engine() {
        let state = test_state();
        let request = Request {
            method: "POST".into(),
            target: "/api/lint".into(),
            headers: HashMap::from([
                ("x-lawlint-token".into(), "test-token".into()),
                ("origin".into(), "http://127.0.0.1:12345".into()),
            ]),
            body: br#"{"text":"We map the landscape of this matter.","noAi":true}"#.to_vec(),
        };
        let response = route(request, &state);
        assert_eq!(response.status, "200 OK");
        let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(
            value["result"]["diagnostics"][0]["ruleId"],
            "core/no-ai-cliches"
        );
        assert_eq!(value["aiStatus"], "AI review skipped: disabled");
    }

    #[test]
    fn file_lint_api_decodes_the_browser_filename_header() {
        let state = test_state();
        let request = Request {
            method: "POST".into(),
            target: "/api/lint-file".into(),
            headers: HashMap::from([
                ("x-lawlint-token".into(), "test-token".into()),
                ("x-lawlint-filename".into(), "r%C3%A9sum%C3%A9.md".into()),
            ]),
            body: b"This is a short draft.".to_vec(),
        };
        let response = route(request, &state);
        assert_eq!(response.status, "200 OK");
        let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(value["sourceName"], "résumé.md");
    }
}

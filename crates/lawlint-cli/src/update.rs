//! CLI versioning, auto update-check, and `self-update` (docs/engine-design.md §11).
//!
//! Distribution store layout, relative to the base URL
//! (`LAWLINT_UPDATE_BASE_URL`, default `https://assets.lawlint.com/downloads`):
//!
//! ```text
//! latest/VERSION                              plaintext "x.y.z"
//! latest/SHA256SUMS
//! releases/v<ver>/lawlint-<target>.<ext>      <ext> = zip on Windows, else tar.gz
//! releases/v<ver>/SHA256SUMS
//! ```
//!
//! The network layer sits behind the [`Fetcher`] trait so the update-check and
//! its pure helpers (semver compare, SHA256SUMS parse, archive naming, the
//! suppression gate, cache freshness) are unit-testable without real I/O.

use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DEFAULT_BASE_URL: &str = "https://assets.lawlint.com/downloads";
/// Auto update-check cadence: at most once per day.
const CHECK_TTL_SECS: u64 = 24 * 60 * 60;
/// Short budget for the once-a-day `latest/VERSION` fetch so it never delays a
/// user's lint result. Downloads (`self-update`) use a generous budget instead.
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// The release asset triple this binary was built for (build.rs).
pub const TARGET: &str = env!("LAWLINT_TARGET");

// ---- config ------------------------------------------------------------

/// Base download URL, trailing slash trimmed.
fn base_url() -> String {
    let raw = std::env::var("LAWLINT_UPDATE_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
    raw.trim_end_matches('/').to_string()
}

fn latest_version_url(base: &str) -> String {
    format!("{base}/latest/VERSION")
}

fn release_url(base: &str, version: &str, file: &str) -> String {
    format!("{base}/releases/v{version}/{file}")
}

/// Archive filename for a target triple: `lawlint-<target>.<ext>`, where
/// `<ext>` is `zip` for a Windows target and `tar.gz` otherwise.
pub fn archive_name(target: &str) -> String {
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("lawlint-{target}.{ext}")
}

/// The extracted binary's filename on a given target.
fn binary_name(target: &str) -> &'static str {
    if target.contains("windows") {
        "lawlint.exe"
    } else {
        "lawlint"
    }
}

// ---- fetcher -----------------------------------------------------------

/// Minimal HTTP GET surface. The real impl uses a blocking `ureq` agent
/// (rustls TLS, no async runtime); tests inject a fake.
pub trait Fetcher {
    fn get_text(&self, url: &str) -> Result<String, String>;
    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String>;
}

/// Blocking `ureq` fetcher with a configurable timeout.
pub struct UreqFetcher {
    agent: ureq::Agent,
}

impl UreqFetcher {
    fn with_timeout(timeout: Duration) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        Self { agent }
    }

    /// Short-budget fetcher for the auto update-check.
    pub fn for_check() -> Self {
        Self::with_timeout(CHECK_TIMEOUT)
    }

    /// Generous-budget fetcher for `self-update` downloads.
    pub fn for_download() -> Self {
        Self::with_timeout(DOWNLOAD_TIMEOUT)
    }
}

impl Fetcher for UreqFetcher {
    fn get_text(&self, url: &str) -> Result<String, String> {
        self.agent
            .get(url)
            .call()
            .map_err(|error| error.to_string())?
            .into_string()
            .map_err(|error| error.to_string())
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|error| error.to_string())?;
        let mut buffer = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut buffer)
            .map_err(|error| error.to_string())?;
        Ok(buffer)
    }
}

// ---- pure helpers ------------------------------------------------------

/// Is `latest` a strictly newer semver than `current`? Leading `v` tolerated.
/// Returns `false` (never errors) when either side fails to parse — the auto
/// check stays silent rather than noisy on a malformed version.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Ok(current), Ok(latest)) => latest > current,
        _ => false,
    }
}

fn parse_version(value: &str) -> Result<semver::Version, semver::Error> {
    semver::Version::parse(value.trim().trim_start_matches('v'))
}

/// Parse a `SHA256SUMS` body into `{ filename -> lowercase-hex }`. Handles both
/// the text form (`<hex>␠␠<name>`) and the binary marker (`<hex>␠*<name>`).
pub fn parse_sha256sums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let hash = parts.next().unwrap_or_default().to_ascii_lowercase();
        let name = parts
            .next()
            .unwrap_or_default()
            .trim_start()
            .trim_start_matches('*');
        if !hash.is_empty() && !name.is_empty() {
            map.insert(name.to_string(), hash);
        }
    }
    map
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Inputs to the auto-check suppression gate (docs §11). Kept as a plain struct
/// so the decision is a pure, table-testable function.
#[derive(Debug, Clone, Copy)]
pub struct GateInputs {
    pub no_update_check_flag: bool,
    pub no_update_check_env: bool,
    pub ci_env: bool,
    pub stderr_is_tty: bool,
    pub json_format: bool,
    pub is_self_update: bool,
}

/// Should the auto update-check run? Suppressed if ANY suppressor is present.
pub fn should_check(gate: &GateInputs) -> bool {
    !(gate.no_update_check_flag
        || gate.no_update_check_env
        || gate.ci_env
        || !gate.stderr_is_tty
        || gate.json_format
        || gate.is_self_update)
}

/// Is a cache entry checked at `last_checked` still fresh at `now`?
fn cache_is_fresh(now: u64, last_checked: u64, ttl: u64) -> bool {
    now.saturating_sub(last_checked) < ttl
}

// ---- cache -------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CacheEntry {
    last_checked: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    latest_version: Option<String>,
}

fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|dir| dir.join("lawlint").join("update-check.json"))
}

fn read_cache() -> Option<CacheEntry> {
    let path = cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_cache(entry: &CacheEntry) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string(entry) {
        let _ = std::fs::write(path, text);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---- auto update-check -------------------------------------------------

/// Runtime knobs threaded from `main`. Everything else the gate reads (env,
/// TTY) is resolved inside [`maybe_notify`].
#[derive(Debug, Clone, Copy)]
pub struct NotifyOptions {
    pub no_update_check_flag: bool,
    pub json_format: bool,
}

/// Core update-check decision, isolated from real time, env, and cache I/O so
/// it can be unit-tested with an injected [`Fetcher`]. Given the current
/// version, base URL, `now`, and the existing cache entry, returns the version
/// to announce (if any) and the cache entry to persist.
fn check_with(
    current: &str,
    fetcher: &dyn Fetcher,
    base: &str,
    now: u64,
    cache: Option<CacheEntry>,
) -> (Option<String>, CacheEntry) {
    // Fresh cache with a known latest: compare, never fetch.
    if let Some(entry) = &cache {
        if cache_is_fresh(now, entry.last_checked, CHECK_TTL_SECS) {
            if let Some(latest) = &entry.latest_version {
                let notice = is_newer(current, latest).then(|| latest.clone());
                return (notice, entry.clone());
            }
        }
    }

    // Stale or missing: fetch with a short timeout.
    match fetcher.get_text(&latest_version_url(base)) {
        Ok(body) => {
            let latest = body.trim().to_string();
            let entry = CacheEntry {
                last_checked: now,
                latest_version: Some(latest.clone()),
            };
            let notice = is_newer(current, &latest).then_some(latest);
            (notice, entry)
        }
        // Any error: silent. Refresh `last_checked` to avoid hammering on
        // repeated failures; keep any previously known latest_version.
        Err(_) => {
            let entry = CacheEntry {
                last_checked: now,
                latest_version: cache.and_then(|entry| entry.latest_version),
            };
            (None, entry)
        }
    }
}

/// Auto update-check: at most once per 24h, cached, silent on any failure,
/// never alters the process exit code or stdout. Call at the very END of a
/// normal lint run, after output is written. Prints a two-line notice to
/// stderr when a newer version is available.
pub fn maybe_notify(current: &str, opts: &NotifyOptions) {
    let gate = GateInputs {
        no_update_check_flag: opts.no_update_check_flag,
        no_update_check_env: std::env::var_os("LAWLINT_NO_UPDATE_CHECK")
            .is_some_and(|value| !value.is_empty()),
        ci_env: std::env::var_os("CI").is_some(),
        stderr_is_tty: std::io::stderr().is_terminal(),
        json_format: opts.json_format,
        is_self_update: false,
    };
    if !should_check(&gate) {
        return;
    }

    let cache = read_cache();
    // Nothing to fetch on a fresh cache — skip building an HTTP agent.
    let need_fetch = !cache.as_ref().is_some_and(|entry| {
        cache_is_fresh(now_unix(), entry.last_checked, CHECK_TTL_SECS)
            && entry.latest_version.is_some()
    });
    let fetcher = UreqFetcher::for_check();
    let (notice, entry) = if need_fetch {
        check_with(current, &fetcher, &base_url(), now_unix(), cache)
    } else {
        // Fresh cache path: reuse check_with's compare logic (it won't fetch).
        check_with(current, &NullFetcher, &base_url(), now_unix(), cache)
    };
    write_cache(&entry);
    if let Some(latest) = notice {
        eprintln!("A new version of lawlint is available: {current} -> {latest}");
        eprintln!("Run `lawlint self-update` to upgrade.");
    }
}

/// A fetcher that always errors — used when the cache is fresh and no network
/// call is wanted, so `check_with` takes its compare-only path.
struct NullFetcher;
impl Fetcher for NullFetcher {
    fn get_text(&self, _url: &str) -> Result<String, String> {
        Err("no fetch".into())
    }
    fn get_bytes(&self, _url: &str) -> Result<Vec<u8>, String> {
        Err("no fetch".into())
    }
}

// ---- self-update -------------------------------------------------------

/// `self-update` subcommand. Returns the process exit code (0 success / already
/// current / --check; 2 on any resolve, verify, extract, or replace failure —
/// always leaving the current binary intact until the atomic replace).
pub fn self_update(
    current: &str,
    check: bool,
    force: bool,
    version: Option<String>,
) -> Result<i32, String> {
    let fetcher = UreqFetcher::for_download();
    self_update_with(current, check, force, version, &fetcher, &base_url())
}

fn self_update_with(
    current: &str,
    check: bool,
    force: bool,
    version: Option<String>,
    fetcher: &dyn Fetcher,
    base: &str,
) -> Result<i32, String> {
    // Resolve the target version: explicit --version, else latest/VERSION.
    let latest = match version {
        Some(value) => value.trim().trim_start_matches('v').to_string(),
        None => fetcher
            .get_text(&latest_version_url(base))
            .map_err(|error| format!("could not resolve the latest version: {error}"))?
            .trim()
            .to_string(),
    };
    if latest.is_empty() {
        return Err("could not resolve the latest version (empty response)".into());
    }
    // Validate both versions so a garbage input fails loudly (exit 2).
    parse_version(current)
        .map_err(|error| format!("invalid current version {current:?}: {error}"))?;
    parse_version(&latest)
        .map_err(|error| format!("invalid target version {latest:?}: {error}"))?;

    let available = is_newer(current, &latest);

    if check {
        println!("Current: {current}");
        println!("Latest:  {latest}");
        if available {
            println!("An update is available. Run `lawlint self-update` to upgrade.");
        } else {
            println!("You are on the latest version.");
        }
        return Ok(0);
    }

    if !available && !force {
        println!("lawlint is already up to date ({current}).");
        return Ok(0);
    }

    // Download the version-pinned archive + its checksums.
    let archive_file = archive_name(TARGET);
    let archive_url = release_url(base, &latest, &archive_file);
    let sums_url = release_url(base, &latest, "SHA256SUMS");
    let archive = fetcher
        .get_bytes(&archive_url)
        .map_err(|error| format!("failed to download {archive_url}: {error}"))?;
    let sums_text = fetcher
        .get_text(&sums_url)
        .map_err(|error| format!("failed to download {sums_url}: {error}"))?;

    // Verify sha256 BEFORE touching the current binary.
    let sums = parse_sha256sums(&sums_text);
    let expected = sums.get(&archive_file).ok_or_else(|| {
        format!("{archive_file} is not listed in SHA256SUMS; aborting (current binary kept)")
    })?;
    let actual = sha256_hex(&archive);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "checksum mismatch for {archive_file} (expected {expected}, got {actual}); \
             aborting (current binary kept)"
        ));
    }

    // Extract the binary next to the current executable, then atomically swap.
    let current_exe = std::env::current_exe()
        .map_err(|error| format!("could not locate the current executable: {error}"))?;
    let dir = current_exe
        .parent()
        .ok_or_else(|| "current executable has no parent directory".to_string())?;
    let temp_path = dir.join(format!(".lawlint-update-{}", std::process::id()));

    let extract_result = extract_binary(&archive, &temp_path);
    if let Err(error) = extract_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }

    let replace_result = self_replace::self_replace(&temp_path);
    let _ = std::fs::remove_file(&temp_path);
    match replace_result {
        Ok(()) => {
            println!("Updated lawlint {current} -> {latest}");
            Ok(0)
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(format!(
            "permission denied replacing {}: {error}\n\
             Re-run with elevated permissions (e.g. sudo), or reinstall lawlint. \
             The current binary was not modified.",
            current_exe.display()
        )),
        Err(error) => Err(format!(
            "failed to replace {}: {error} (current binary kept)",
            current_exe.display()
        )),
    }
}

/// Extract the `lawlint` binary from a `.tar.gz` release archive to `dest`.
#[cfg(not(windows))]
fn extract_binary(archive: &[u8], dest: &Path) -> Result<(), String> {
    use std::io::Cursor;
    let wanted = binary_name(TARGET);
    let decoder = flate2::read::GzDecoder::new(Cursor::new(archive));
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|error| format!("failed to read archive: {error}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("failed to read archive entry: {error}"))?;
        let path = entry
            .path()
            .map_err(|error| format!("failed to read archive entry path: {error}"))?;
        if path.file_name().and_then(|name| name.to_str()) == Some(wanted) {
            entry
                .unpack(dest)
                .map_err(|error| format!("failed to extract {wanted}: {error}"))?;
            set_executable(dest)?;
            return Ok(());
        }
    }
    Err(format!("{wanted} not found in the release archive"))
}

/// Extract the `lawlint.exe` binary from a `.zip` release archive to `dest`.
#[cfg(windows)]
fn extract_binary(archive: &[u8], dest: &Path) -> Result<(), String> {
    use std::io::{Cursor, Write};
    let wanted = binary_name(TARGET);
    let mut zip = zip::ZipArchive::new(Cursor::new(archive))
        .map_err(|error| format!("failed to read archive: {error}"))?;
    for index in 0..zip.len() {
        let mut file = zip
            .by_index(index)
            .map_err(|error| format!("failed to read archive entry: {error}"))?;
        let matches = file
            .enclosed_name()
            .and_then(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            })
            .is_some_and(|name| name == wanted);
        if matches {
            let mut out = std::fs::File::create(dest)
                .map_err(|error| format!("failed to create {}: {error}", dest.display()))?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)
                .map_err(|error| format!("failed to extract {wanted}: {error}"))?;
            out.write_all(&buffer)
                .map_err(|error| format!("failed to write {wanted}: {error}"))?;
            return Ok(());
        }
    }
    Err(format!("{wanted} not found in the release archive"))
}

#[cfg(not(windows))]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).map_err(|error| error.to_string())
}

// ---- tests -------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    // ---- semver compare ----

    #[test]
    fn is_newer_covers_ordering_equal_and_prerelease() {
        assert!(is_newer("0.2.0", "0.3.0"));
        assert!(is_newer("0.2.0", "0.2.1"));
        assert!(!is_newer("0.3.0", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0")); // equal is not newer
                                              // Leading v tolerated on either side.
        assert!(is_newer("v0.2.0", "v0.3.0"));
        // Pre-release precedes its release: 0.3.0-rc.1 < 0.3.0.
        assert!(is_newer("0.3.0-rc.1", "0.3.0"));
        assert!(!is_newer("0.3.0", "0.3.0-rc.1"));
        assert!(is_newer("0.3.0-rc.1", "0.3.0-rc.2"));
        // Unparseable → never newer (silent).
        assert!(!is_newer("garbage", "0.3.0"));
        assert!(!is_newer("0.2.0", "not-a-version"));
    }

    // ---- SHA256SUMS parse ----

    #[test]
    fn parse_sha256sums_text_and_binary_markers() {
        let text = "\
aaaa  lawlint-aarch64-apple-darwin.tar.gz\n\
bbbb *lawlint-x86_64-pc-windows-msvc.zip\n\
\n\
CCCC  lawlint-x86_64-unknown-linux-gnu.tar.gz\n";
        let map = parse_sha256sums(text);
        assert_eq!(map.len(), 3);
        assert_eq!(
            map.get("lawlint-aarch64-apple-darwin.tar.gz"),
            Some(&"aaaa".to_string())
        );
        // `*` binary marker stripped from the filename.
        assert_eq!(
            map.get("lawlint-x86_64-pc-windows-msvc.zip"),
            Some(&"bbbb".to_string())
        );
        // Hash lowercased.
        assert_eq!(
            map.get("lawlint-x86_64-unknown-linux-gnu.tar.gz"),
            Some(&"cccc".to_string())
        );
    }

    #[test]
    fn sha256_hex_known_vector() {
        // sha256("") well-known digest.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ---- archive naming ----

    #[test]
    fn archive_name_per_target() {
        assert_eq!(
            archive_name("aarch64-apple-darwin"),
            "lawlint-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            archive_name("x86_64-unknown-linux-gnu"),
            "lawlint-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            archive_name("x86_64-pc-windows-msvc"),
            "lawlint-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(binary_name("x86_64-pc-windows-msvc"), "lawlint.exe");
        assert_eq!(binary_name("aarch64-apple-darwin"), "lawlint");
    }

    // ---- URL construction ----

    #[test]
    fn urls_use_base_and_version() {
        let base = "https://example.test/downloads";
        assert_eq!(
            latest_version_url(base),
            "https://example.test/downloads/latest/VERSION"
        );
        assert_eq!(
            release_url(base, "0.3.0", "SHA256SUMS"),
            "https://example.test/downloads/releases/v0.3.0/SHA256SUMS"
        );
    }

    // ---- suppression gate ----

    fn gate(
        flag: bool,
        env: bool,
        ci: bool,
        tty: bool,
        json: bool,
        self_update: bool,
    ) -> GateInputs {
        GateInputs {
            no_update_check_flag: flag,
            no_update_check_env: env,
            ci_env: ci,
            stderr_is_tty: tty,
            json_format: json,
            is_self_update: self_update,
        }
    }

    #[test]
    fn should_check_gate_table() {
        // Only case where the check runs: nothing suppresses and stderr is a TTY.
        assert!(should_check(&gate(false, false, false, true, false, false)));
        // Each suppressor individually blocks it.
        assert!(!should_check(&gate(true, false, false, true, false, false))); // --no-update-check
        assert!(!should_check(&gate(false, true, false, true, false, false))); // env
        assert!(!should_check(&gate(false, false, true, true, false, false))); // CI
        assert!(!should_check(&gate(
            false, false, false, false, false, false
        ))); // not a TTY
        assert!(!should_check(&gate(false, false, false, true, true, false))); // json
        assert!(!should_check(&gate(false, false, false, true, false, true))); // self-update
    }

    // ---- cache freshness ----

    #[test]
    fn cache_freshness_compare() {
        let ttl = CHECK_TTL_SECS;
        assert!(cache_is_fresh(1000, 1000, ttl)); // same instant
        assert!(cache_is_fresh(1000 + ttl - 1, 1000, ttl)); // just under
        assert!(!cache_is_fresh(1000 + ttl, 1000, ttl)); // exactly TTL = stale
        assert!(!cache_is_fresh(1000 + ttl + 5, 1000, ttl));
        // Clock skew (now < last_checked) does not underflow → treated fresh.
        assert!(cache_is_fresh(500, 1000, ttl));
    }

    // ---- injected Fetcher end-to-end of the check decision ----

    struct FakeFetcher {
        result: Result<String, String>,
        calls: RefCell<usize>,
    }
    impl FakeFetcher {
        fn ok(body: &str) -> Self {
            Self {
                result: Ok(body.to_string()),
                calls: RefCell::new(0),
            }
        }
        fn err() -> Self {
            Self {
                result: Err("network down".into()),
                calls: RefCell::new(0),
            }
        }
    }
    impl Fetcher for FakeFetcher {
        fn get_text(&self, _url: &str) -> Result<String, String> {
            *self.calls.borrow_mut() += 1;
            self.result.clone()
        }
        fn get_bytes(&self, _url: &str) -> Result<Vec<u8>, String> {
            unreachable!("update-check never fetches bytes")
        }
    }

    #[test]
    fn check_newer_version_produces_notice() {
        let fetcher = FakeFetcher::ok("0.3.0\n");
        let (notice, entry) = check_with("0.2.0", &fetcher, "https://b", 5000, None);
        assert_eq!(notice, Some("0.3.0".to_string()));
        assert_eq!(entry.latest_version, Some("0.3.0".to_string()));
        assert_eq!(entry.last_checked, 5000);
        assert_eq!(*fetcher.calls.borrow(), 1);
    }

    #[test]
    fn check_same_or_older_is_silent() {
        let (notice, _) = check_with("0.3.0", &FakeFetcher::ok("0.3.0"), "https://b", 1, None);
        assert_eq!(notice, None);
        let (notice, _) = check_with("0.3.0", &FakeFetcher::ok("0.2.0"), "https://b", 1, None);
        assert_eq!(notice, None);
    }

    #[test]
    fn check_fetch_error_is_silent_and_refreshes_timestamp() {
        let prior = CacheEntry {
            last_checked: 100,
            latest_version: Some("0.2.5".into()),
        };
        // now is past the TTL so the entry is stale → a fetch is attempted.
        let now = 100 + CHECK_TTL_SECS + 1;
        let (notice, entry) =
            check_with("0.2.0", &FakeFetcher::err(), "https://b", now, Some(prior));
        // Silent on error, never panics.
        assert_eq!(notice, None);
        // last_checked bumped; previously known latest kept.
        assert_eq!(entry.last_checked, now);
        assert_eq!(entry.latest_version, Some("0.2.5".to_string()));
    }

    #[test]
    fn check_fresh_cache_does_not_fetch() {
        let fetcher = FakeFetcher::ok("9.9.9"); // must NOT be consulted
        let fresh = CacheEntry {
            last_checked: 1000,
            latest_version: Some("0.3.0".into()),
        };
        let (notice, _) = check_with("0.2.0", &fetcher, "https://b", 1500, Some(fresh));
        assert_eq!(notice, Some("0.3.0".to_string())); // from cache, not the fetch
        assert_eq!(*fetcher.calls.borrow(), 0);
    }

    #[test]
    fn check_stale_cache_refetches() {
        let fetcher = FakeFetcher::ok("0.4.0");
        let stale = CacheEntry {
            last_checked: 1000,
            latest_version: Some("0.3.0".into()),
        };
        let now = 1000 + CHECK_TTL_SECS + 1;
        let (notice, entry) = check_with("0.2.0", &fetcher, "https://b", now, Some(stale));
        assert_eq!(notice, Some("0.4.0".to_string()));
        assert_eq!(entry.latest_version, Some("0.4.0".to_string()));
        assert_eq!(*fetcher.calls.borrow(), 1);
    }

    // ---- self-update decision paths (no real download/replace) ----

    struct MapFetcher {
        responses: HashMap<String, String>,
    }
    impl Fetcher for MapFetcher {
        fn get_text(&self, url: &str) -> Result<String, String> {
            self.responses
                .get(url)
                .cloned()
                .ok_or_else(|| format!("404 {url}"))
        }
        fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
            self.responses
                .get(url)
                .map(|s| s.clone().into_bytes())
                .ok_or_else(|| format!("404 {url}"))
        }
    }

    #[test]
    fn self_update_check_reports_without_downloading() {
        let base = "https://b";
        let fetcher = MapFetcher {
            responses: [(latest_version_url(base), "0.3.0".to_string())]
                .into_iter()
                .collect(),
        };
        // --check with a newer latest: exit 0, no download attempted.
        let code = self_update_with("0.2.0", true, false, None, &fetcher, base).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn self_update_already_current_short_circuits() {
        let base = "https://b";
        let fetcher = MapFetcher {
            responses: HashMap::new(),
        };
        // Explicit --version equal to current, not forced: no fetch, exit 0.
        let code =
            self_update_with("0.3.0", false, false, Some("0.3.0".into()), &fetcher, base).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn self_update_resolve_failure_is_config_error() {
        let base = "https://b";
        let fetcher = MapFetcher {
            responses: HashMap::new(), // latest/VERSION missing
        };
        assert!(self_update_with("0.2.0", false, false, None, &fetcher, base).is_err());
    }

    #[test]
    fn self_update_checksum_mismatch_aborts() {
        let base = "https://b";
        let archive_file = archive_name(TARGET);
        let archive_url = release_url(base, "0.3.0", &archive_file);
        let sums_url = release_url(base, "0.3.0", "SHA256SUMS");
        // SHA256SUMS lists a hash that does NOT match the archive bytes.
        let sums = format!("deadbeef  {archive_file}\n");
        let fetcher = MapFetcher {
            responses: [
                (archive_url, "not-a-real-archive".to_string()),
                (sums_url, sums),
            ]
            .into_iter()
            .collect(),
        };
        let error = self_update_with("0.2.0", false, false, Some("0.3.0".into()), &fetcher, base)
            .unwrap_err();
        assert!(error.contains("checksum mismatch"), "got: {error}");
    }

    #[test]
    fn self_update_missing_checksum_entry_aborts() {
        let base = "https://b";
        let archive_file = archive_name(TARGET);
        let archive_url = release_url(base, "0.3.0", &archive_file);
        let sums_url = release_url(base, "0.3.0", "SHA256SUMS");
        let fetcher = MapFetcher {
            responses: [
                (archive_url, "bytes".to_string()),
                (sums_url, "aaaa  some-other-file.tar.gz\n".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let error = self_update_with("0.2.0", false, false, Some("0.3.0".into()), &fetcher, base)
            .unwrap_err();
        assert!(error.contains("not listed in SHA256SUMS"), "got: {error}");
    }
}

//! User-level credential store for hosted AI providers.
//!
//! API keys must never land in `.lawlint/config.json` (it gets committed),
//! so `lawlint init` writes them to `~/.config/lawlint/credentials` — a
//! dotenv-style file of `NAME=value` lines keyed by each provider's
//! environment-variable name, created with mode 0600. Lookups prefer the
//! process environment, so an exported variable (CI, one-off overrides)
//! always wins over the file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// `$LAWLINT_CREDENTIALS`, else `$XDG_CONFIG_HOME/lawlint/credentials`,
/// else `~/.config/lawlint/credentials` (`%USERPROFILE%` on Windows).
/// `None` only when no home directory can be determined.
pub fn default_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LAWLINT_CREDENTIALS") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        if !config.is_empty() {
            return Some(PathBuf::from(config).join("lawlint").join("credentials"));
        }
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    if home.is_empty() {
        return None;
    }
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("lawlint")
            .join("credentials"),
    )
}

/// Resolve `name` (an env-var name like `ANTHROPIC_API_KEY`): the process
/// environment wins; the credential file is the fallback.
pub fn lookup(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name) {
        if !value.is_empty() {
            return Some(value);
        }
    }
    lookup_at(&default_path()?, name)
}

/// File-only lookup (no environment), for tests and callers that already
/// resolved the path.
pub fn lookup_at(path: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    parse(&text).remove(name)
}

/// Merge `entries` into the credential file at the default path, creating
/// it (0600) if needed. Returns the path written.
pub fn store(entries: &[(String, String)]) -> Result<PathBuf, String> {
    let path = default_path().ok_or("could not determine a home directory for credentials")?;
    store_at(&path, entries)?;
    Ok(path)
}

/// Merge `entries` into the credential file at `path`. Existing entries not
/// named in `entries` are preserved; the file ends up mode 0600 on Unix.
pub fn store_at(path: &Path, entries: &[(String, String)]) -> Result<(), String> {
    let mut map = fs::read_to_string(path)
        .map(|text| parse(&text))
        .unwrap_or_default();
    for (name, value) in entries {
        map.insert(name.clone(), value.clone());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    fs::write(path, render(&map))
        .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("failed to chmod {}: {error}", path.display()))?;
    }
    Ok(())
}

/// `NAME=value` lines; `#` comments and blank lines are ignored. Values keep
/// internal whitespace (keys are trimmed).
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            let name = name.trim();
            if !name.is_empty() {
                map.insert(name.to_string(), value.trim().to_string());
            }
        }
    }
    map
}

fn render(map: &BTreeMap<String, String>) -> String {
    let mut out =
        String::from("# lawlint credentials — written by `lawlint init`. Keep private.\n");
    for (name, value) in map {
        out.push_str(name);
        out.push('=');
        out.push_str(value);
        out.push('\n');
    }
    out
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skips_comments_and_malformed_lines() {
        let text = "# comment\n\nA_KEY=abc123\nnot a pair\n B = spaced value \n=novalue\n";
        let map = parse(text);
        assert_eq!(map.get("A_KEY").unwrap(), "abc123");
        assert_eq!(map.get("B").unwrap(), "spaced value");
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn store_at_merges_and_sets_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("credentials");

        store_at(&path, &[("ANTHROPIC_API_KEY".into(), "sk-one".into())]).unwrap();
        assert_eq!(lookup_at(&path, "ANTHROPIC_API_KEY").unwrap(), "sk-one");

        // Second store merges: the old entry survives, the named one updates.
        store_at(
            &path,
            &[
                ("OPENAI_API_KEY".into(), "sk-two".into()),
                ("ANTHROPIC_API_KEY".into(), "sk-three".into()),
            ],
        )
        .unwrap();
        assert_eq!(lookup_at(&path, "ANTHROPIC_API_KEY").unwrap(), "sk-three");
        assert_eq!(lookup_at(&path, "OPENAI_API_KEY").unwrap(), "sk-two");
        assert_eq!(lookup_at(&path, "MISSING"), None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn lookup_at_missing_file_is_none() {
        assert_eq!(lookup_at(Path::new("/no/such/credentials"), "X"), None);
    }
}

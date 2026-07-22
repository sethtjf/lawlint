//! User-level credential store for hosted AI providers.
//!
//! API keys must never land in `.lawlint/config.json` (it gets committed),
//! so `lawlint init` writes them to `~/.lawlint/credentials` — a dotenv-style
//! file of `NAME=value` lines keyed by each provider's environment-variable
//! name, created with mode 0600. Lookups prefer the process environment, so an
//! exported variable (CI, one-off overrides) always wins over the file.
//!
//! `~/.lawlint/` mirrors the project-local `.lawlint/` directory, so there is
//! one name to learn for both scopes. Releases before 0.8 used
//! `~/.config/lawlint/`; that path is still read, and [`migrate_legacy`] moves
//! it across the first time anything is written.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// The user-level lawlint directory: `$LAWLINT_HOME`, else `~/.lawlint`
/// (`%USERPROFILE%` on Windows). `None` only when no home directory can be
/// determined. Holds the credential store and — for users who lint documents
/// that live outside any project — `config.json`.
pub fn config_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("LAWLINT_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".lawlint"))
}

/// The pre-0.8 user-level directory: `$XDG_CONFIG_HOME/lawlint`, else
/// `~/.config/lawlint`. Read-only fallback so an existing install keeps
/// working before it is migrated.
pub fn legacy_config_home() -> Option<PathBuf> {
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME") {
        if !config.is_empty() {
            return Some(PathBuf::from(config).join("lawlint"));
        }
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".config").join("lawlint"))
}

/// Where a user-level file lives *for reading*: the current location if it
/// exists, else the legacy one if that does, else the current location.
pub fn resolve_user_file(name: &str) -> Option<PathBuf> {
    let current = config_home().map(|dir| dir.join(name));
    if let Some(path) = &current {
        if path.is_file() {
            return current;
        }
    }
    if let Some(legacy) = legacy_config_home().map(|dir| dir.join(name)) {
        if legacy.is_file() {
            return Some(legacy);
        }
    }
    current
}

/// `$LAWLINT_CREDENTIALS`, else the credential file under [`config_home`],
/// falling back to the legacy directory while only that copy exists.
/// `None` only when no home directory can be determined.
pub fn default_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LAWLINT_CREDENTIALS") {
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    resolve_user_file("credentials")
}

/// Move a pre-0.8 `~/.config/lawlint/<name>` to `~/.lawlint/<name>`, once.
///
/// Returns `Some((from, to))` when a move happened, so the caller can tell the
/// user where their file went — silently relocating an API key is exactly the
/// kind of thing that should be stated out loud. No-ops when the destination
/// already exists (the new file wins; the stale copy is left alone rather than
/// deleted), when there is nothing to move, or when `$LAWLINT_HOME` /
/// `$LAWLINT_CREDENTIALS` put the file somewhere explicit.
pub fn migrate_legacy(name: &str) -> Option<(PathBuf, PathBuf)> {
    if name == "credentials" && std::env::var_os("LAWLINT_CREDENTIALS").is_some() {
        return None;
    }
    migrate_between(&legacy_config_home()?, &config_home()?, name)
}

/// [`migrate_legacy`] against explicit directories, reading no environment.
///
/// Every caller that is not the real CLI entry point uses this: a function that
/// discovers `$HOME` on its own will, sooner or later, be called from a unit
/// test and move the developer's own credentials. Requiring the directories
/// makes the safe thing the default and the destructive thing deliberate.
pub fn migrate_between(
    legacy_dir: &Path,
    new_dir: &Path,
    name: &str,
) -> Option<(PathBuf, PathBuf)> {
    let to = new_dir.join(name);
    if to.exists() {
        return None;
    }
    let from = legacy_dir.join(name);
    if !from.is_file() || from == to {
        return None;
    }
    fs::create_dir_all(to.parent()?).ok()?;
    // rename() fails across filesystems; fall back to copy-then-remove so the
    // key is never left only in a half-written destination.
    if fs::rename(&from, &to).is_err() {
        fs::copy(&from, &to).ok()?;
        let _ = fs::remove_file(&from);
    }
    #[cfg(unix)]
    if name == "credentials" {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&to, fs::Permissions::from_mode(0o600));
    }
    Some((from, to))
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
/// it (0600) if needed. Returns the path written, plus the legacy path it was
/// migrated from if this call moved it.
///
/// Writing is the moment to migrate: it is user-initiated (`lawlint init`), so
/// the move can be reported, and it keeps every key in one file instead of
/// splitting new keys from old ones across two locations.
pub fn store(entries: &[(String, String)]) -> Result<(PathBuf, Option<PathBuf>), String> {
    let moved = migrate_legacy("credentials").map(|(from, _)| from);
    let path = default_path().ok_or("could not determine a home directory for credentials")?;
    store_at(&path, entries)?;
    Ok((path, moved))
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

    // These exercise `migrate_between`, which takes explicit directories. The
    // env-reading `migrate_legacy` wrapper is deliberately untested here: a
    // unit test that mutates process-wide $HOME state has already, once, moved
    // the developer's real credential file.

    #[test]
    fn migrate_moves_the_legacy_file_once_and_keeps_the_keys() {
        let dir = tempfile::tempdir().unwrap();
        let (legacy, new) = (dir.path().join("old"), dir.path().join("new"));
        let from = legacy.join("credentials");
        store_at(&from, &[("ANTHROPIC_API_KEY".into(), "sk-old".into())]).unwrap();

        let (moved_from, moved_to) = migrate_between(&legacy, &new, "credentials").unwrap();
        assert_eq!(moved_from, from);
        assert_eq!(moved_to, new.join("credentials"));
        assert!(!from.exists(), "legacy file should be moved, not copied");
        assert_eq!(
            lookup_at(&new.join("credentials"), "ANTHROPIC_API_KEY").unwrap(),
            "sk-old"
        );

        // Idempotent: a second call has nothing left to move.
        assert!(migrate_between(&legacy, &new, "credentials").is_none());
    }

    #[test]
    fn migrate_leaves_an_existing_new_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (legacy, new) = (dir.path().join("old"), dir.path().join("new"));
        store_at(
            &legacy.join("credentials"),
            &[("ANTHROPIC_API_KEY".into(), "sk-old".into())],
        )
        .unwrap();
        store_at(
            &new.join("credentials"),
            &[("ANTHROPIC_API_KEY".into(), "sk-new".into())],
        )
        .unwrap();

        assert!(migrate_between(&legacy, &new, "credentials").is_none());
        // The newer file wins and is not clobbered by the stale one.
        assert_eq!(
            lookup_at(&new.join("credentials"), "ANTHROPIC_API_KEY").unwrap(),
            "sk-new"
        );
        assert!(
            legacy.join("credentials").exists(),
            "stale copy is left, not deleted"
        );
    }

    #[test]
    fn migrate_is_a_no_op_when_there_is_nothing_to_move() {
        let dir = tempfile::tempdir().unwrap();
        let (legacy, new) = (dir.path().join("old"), dir.path().join("new"));
        assert!(migrate_between(&legacy, &new, "credentials").is_none());
        assert!(
            !new.exists(),
            "must not create a directory for a file that does not exist"
        );
    }

    #[test]
    fn migrate_preserves_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let (legacy, new) = (dir.path().join("old"), dir.path().join("new"));
        store_at(
            &legacy.join("credentials"),
            &[("ANTHROPIC_API_KEY".into(), "sk".into())],
        )
        .unwrap();
        migrate_between(&legacy, &new, "credentials").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(new.join("credentials"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

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

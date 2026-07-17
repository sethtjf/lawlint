//! `DiskCache` — `lawlint_core::JudgeCache` backed by sha-keyed JSON files
//! under `~/.cache/lawlint/judge/`. Corrupt or unreadable entries are cache
//! misses, never errors.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use lawlint_core::{JudgeCache, JudgeFinding};
use sha2::{Digest, Sha256};

pub struct DiskCache {
    dir: PathBuf,
}

impl DiskCache {
    /// Cache under the default location: `$XDG_CACHE_HOME/lawlint/judge/`
    /// or `~/.cache/lawlint/judge/`. The directory is created lazily on the
    /// first `put`.
    pub fn new() -> anyhow::Result<Self> {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|home| PathBuf::from(home).join(".cache"))
            })
            .ok_or_else(|| anyhow::anyhow!("cannot locate a cache directory (no HOME set)"))?;
        Ok(DiskCache::at(base.join("lawlint").join("judge")))
    }

    /// Cache rooted at an explicit directory (tests, custom layouts).
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        DiskCache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Keys from core are already sha256 hex, but the trait accepts any
    /// string — hash defensively so every key maps to a safe filename.
    fn path_for(&self, key: &str) -> PathBuf {
        let digest = Sha256::digest(key.as_bytes());
        let mut name = String::with_capacity(69);
        for byte in digest {
            let _ = write!(name, "{byte:02x}");
        }
        name.push_str(".json");
        self.dir.join(name)
    }
}

impl JudgeCache for DiskCache {
    fn get(&self, key: &str) -> Option<Vec<JudgeFinding>> {
        let bytes = fs::read(self.path_for(key)).ok()?;
        // Corrupt entries are misses; run_judge will re-evaluate and `put`
        // a fresh entry over them.
        serde_json::from_slice(&bytes).ok()
    }

    fn put(&self, key: &str, findings: &[JudgeFinding]) {
        // Cache writes are best-effort: failure to persist must never fail
        // a lint run.
        let Ok(json) = serde_json::to_vec(findings) else {
            return;
        };
        if fs::create_dir_all(&self.dir).is_err() {
            return;
        }
        let path = self.path_for(key);
        // Write via a unique temp file + rename so concurrent lawlint
        // processes never observe (or clobber each other with) partial files.
        let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
        if fs::write(&tmp, &json).is_ok() && fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

// ------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(rule: &str, quote: &str) -> JudgeFinding {
        JudgeFinding {
            rule: rule.to_string(),
            quote: quote.to_string(),
            explanation: "why".to_string(),
            confidence: 0.7,
            suggested_rewrite: Some("rewrite".to_string()),
        }
    }

    #[test]
    fn round_trip_get_put() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::at(dir.path());
        assert!(cache.get("k1").is_none());

        cache.put("k1", &[finding("core/empty-hedge", "could perhaps")]);
        let got = cache.get("k1").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rule, "core/empty-hedge");
        assert_eq!(got[0].quote, "could perhaps");
        assert_eq!(got[0].confidence, 0.7);
        assert_eq!(got[0].suggested_rewrite.as_deref(), Some("rewrite"));

        // Empty findings (a clean chunk) round-trip too — a cached "no
        // findings" must not look like a miss.
        cache.put("k2", &[]);
        assert_eq!(cache.get("k2").unwrap().len(), 0);
        // Distinct keys are distinct files.
        assert_eq!(cache.get("k1").unwrap().len(), 1);
    }

    #[test]
    fn overwrite_replaces_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::at(dir.path());
        cache.put("k", &[finding("core/a", "one")]);
        cache.put("k", &[finding("core/b", "two")]);
        let got = cache.get("k").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rule, "core/b");
    }

    #[test]
    fn corrupt_entry_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::at(dir.path());
        cache.put("k", &[finding("core/a", "q")]);
        let path = cache.path_for("k");
        fs::write(&path, b"{ not json !!!").unwrap();
        assert!(cache.get("k").is_none());
        // Valid JSON of the wrong shape is also a miss.
        fs::write(&path, b"{\"rule\": \"x\"}").unwrap();
        assert!(cache.get("k").is_none());
        // A fresh put recovers.
        cache.put("k", &[finding("core/a", "q")]);
        assert_eq!(cache.get("k").unwrap().len(), 1);
    }

    #[test]
    fn arbitrary_keys_map_to_safe_filenames() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::at(dir.path());
        let nasty = "../../etc/passwd\0/weird key ⚖️";
        cache.put(nasty, &[finding("core/a", "q")]);
        assert_eq!(cache.get(nasty).unwrap().len(), 1);
        // The file landed inside the cache dir, sha-named.
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let name = entries[0].as_ref().unwrap().file_name();
        let name = name.to_string_lossy().to_string();
        assert!(name.ends_with(".json"));
        assert_eq!(name.len(), 64 + ".json".len());
    }

    #[test]
    fn missing_dir_is_miss_not_error() {
        let cache = DiskCache::at("/nonexistent/lawlint-test-cache");
        assert!(cache.get("anything").is_none());
        // put into an uncreatable dir must not panic.
        cache.put("anything", &[]);
    }

    #[test]
    fn works_through_core_run_judge() {
        use lawlint_core::{run_judge, JudgeRequest, MockJudge, RuleId, TextRange};
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::at(dir.path());
        let text = "It could perhaps be argued that the claim fails.";
        let req = JudgeRequest {
            chunk_range: TextRange { start: 0, end: text.len() },
            chunk_text: text.to_string(),
            rules: vec![RuleId("core/empty-hedge".to_string())],
            prompt: "p".to_string(),
            cache_key_base: "base".to_string(),
        };
        let judge = MockJudge::new().respond(
            text,
            vec![finding("core/empty-hedge", "could perhaps be argued")],
        );
        let (out1, stats1) = run_judge(&judge, Some(&cache), std::slice::from_ref(&req), text);
        assert_eq!(judge.calls(), 1);
        assert_eq!(stats1.cache_hits, 0);
        // Second run: disk cache hit, judge not called again.
        let (out2, stats2) = run_judge(&judge, Some(&cache), std::slice::from_ref(&req), text);
        assert_eq!(judge.calls(), 1);
        assert_eq!(stats2.cache_hits, 1);
        assert_eq!(out1.len(), out2.len());
        assert_eq!(out1[0].2, out2[0].2);
    }
}

//! RuleSet: packages, defs, aliases. [agent D]
//!
//! Aliases: bare `name` resolves to `pkg/name` when unambiguous (legacy flat
//! ids keep working in enable/disable/severity/thresholds/suppression).
//! Ambiguity is a config error, silently preferring nothing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use include_dir::{include_dir, Dir};

use crate::config::LintOptions;
use crate::engines::{DensityEngine, LeadingEngine, PhraseEngine, StatisticalEngine};
use crate::error::LoadError;
use crate::judge::{Granularity, RubricFragment};
use crate::loader::{self, parse_manifest, parse_rule, PackageManifest, RuleDef};
use crate::rule::{Interests, Rule, RuleMeta};
use crate::types::{Intent, RuleId, Scope, Severity};

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// The embedded built-in package (`crates/lawlint-core/builtin/`):
/// `style.yaml` + `rules/*.yaml`, loaded through the same loader as user
/// packages.
pub(crate) static BUILTIN_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/builtin");

/// Soft (inferential/tier-3) declarative rule: no runtime checks, just a
/// `RubricFragment` for the AI judge pipeline to pick up via `Rule::rubric()`.
pub struct InferentialRule {
    meta: RuleMeta,
    fragment: RubricFragment,
}

impl InferentialRule {
    pub fn from_def(meta: RuleMeta, def: &RuleDef) -> Self {
        let fragment = RubricFragment {
            rule: meta.id.clone(),
            severity: meta.severity,
            granularity: def
                .granularity
                .as_deref()
                .and_then(loader::parse_granularity)
                .unwrap_or(Granularity::Sentence),
            rubric: def.rubric.clone().unwrap_or_default(),
            flag_examples: def.flag_examples.clone(),
            pass_examples: def.pass_examples.clone(),
        };
        InferentialRule { meta, fragment }
    }
}

impl Rule for InferentialRule {
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }

    fn interests(&self) -> Interests {
        Interests::default()
    }

    fn rubric(&self) -> Option<&RubricFragment> {
        Some(&self.fragment)
    }
}

/// Parsed rule definitions + alias map. Instantiates fresh `Rule` boxes per
/// lint run.
#[derive(Debug, Default, Clone)]
pub struct RuleSet {
    /// Validated defs with their derived metas, keyed by full id, plus the
    /// source file for error context.
    defs: Vec<(RuleMeta, RuleDef, String)>,
    /// bare-name → full id (only where unambiguous).
    aliases: HashMap<String, RuleId>,
}

/// Derive the `RuleMeta` for a validated def. Infallible: `parse_rule` has
/// already rejected anything these unwraps would trip on.
fn build_meta(package: &str, def: &RuleDef) -> RuleMeta {
    RuleMeta {
        id: RuleId(format!("{package}/{}", def.id)),
        tier: loader::engine_tier(&def.engine).expect("engine validated by loader"),
        scope: def
            .scope
            .as_deref()
            .and_then(loader::parse_scope)
            .unwrap_or(Scope::Text),
        severity: def
            .severity
            .as_deref()
            .and_then(loader::parse_severity)
            .unwrap_or(Severity::Warning),
        intent: def
            .intent
            .as_deref()
            .and_then(loader::parse_intent)
            .unwrap_or(Intent::Detection),
        description: def.description.clone().unwrap_or_default(),
        docs_url: def
            .docs
            .clone()
            .unwrap_or_else(|| format!("https://lawlint.com/rules/{}", def.id)),
        rationale: def.rationale.clone(),
        examples: def.examples.clone(),
    }
}

impl RuleSet {
    /// The embedded built-in package.
    ///
    /// Panics if the embedded package fails validation — that is a build bug,
    /// not a runtime condition. Tolerates `builtin/rules/` being empty or
    /// partially populated; validation runs on whatever is present.
    pub fn built_in() -> RuleSet {
        Self::from_embedded(&BUILTIN_DIR)
            .unwrap_or_else(|e| panic!("built-in rule package is invalid: {e}"))
    }

    fn from_embedded(dir: &Dir<'static>) -> Result<RuleSet, LoadError> {
        let manifest_file = dir.get_file("style.yaml").ok_or_else(|| LoadError::Io {
            path: "builtin/style.yaml".to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "embedded file not found"),
        })?;
        let manifest_text = manifest_file
            .contents_utf8()
            .ok_or_else(|| LoadError::Yaml {
                file: "builtin/style.yaml".to_string(),
                message: "file is not valid UTF-8".to_string(),
            })?;
        let manifest = parse_manifest("builtin/style.yaml", manifest_text)?;

        let mut rules: Vec<(String, RuleDef)> = Vec::new();
        if let Some(rules_dir) = dir.get_dir("rules") {
            let mut files: Vec<_> = rules_dir
                .files()
                .filter(|f| {
                    matches!(
                        f.path().extension().and_then(|e| e.to_str()),
                        Some("yaml") | Some("yml")
                    )
                })
                .collect();
            files.sort_by_key(|f| f.path().to_path_buf());
            for f in files {
                let name = format!("builtin/{}", f.path().display());
                let text = f.contents_utf8().ok_or_else(|| LoadError::Yaml {
                    file: name.clone(),
                    message: "file is not valid UTF-8".to_string(),
                })?;
                let raw = loader::parse_rule_def(&name, text)?;
                let def = if let Some(skill) = raw.skill.clone() {
                    let skill_path = normalize_path(
                        f.path()
                            .parent()
                            .unwrap_or_else(|| Path::new(""))
                            .join(skill),
                    );
                    let skill_name = format!("builtin/{}", skill_path.display());
                    let skill_file = dir.get_file(&skill_path).ok_or_else(|| LoadError::Io {
                        path: skill_name.clone(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            "referenced skill file not found",
                        ),
                    })?;
                    let skill_text = skill_file.contents_utf8().ok_or_else(|| LoadError::Yaml {
                        file: skill_name.clone(),
                        message: "file is not valid UTF-8".to_string(),
                    })?;
                    loader::parse_rule_with_skill(&name, text, &skill_name, skill_text)?
                } else {
                    parse_rule(&name, text)?
                };
                rules.push((name.clone(), def));
            }
        }
        Self::from_parts(&manifest, rules)
    }

    /// Load a package directory (`style.yaml` + `rules/*.yaml`). Skill files
    /// referenced by inferential YAML rules are resolved relative to them.
    /// The rules directory may be absent or empty; the manifest is required.
    pub fn load_dir(path: &Path) -> Result<RuleSet, LoadError> {
        let manifest_path = path.join("style.yaml");
        let manifest_name = manifest_path.display().to_string();
        let manifest_text = std::fs::read_to_string(&manifest_path).map_err(|e| LoadError::Io {
            path: manifest_name.clone(),
            source: e,
        })?;
        let manifest = parse_manifest(&manifest_name, &manifest_text)?;

        let mut rules: Vec<(String, RuleDef)> = Vec::new();
        let rules_dir = path.join("rules");
        if rules_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&rules_dir)
                .map_err(|e| LoadError::Io {
                    path: rules_dir.display().to_string(),
                    source: e,
                })?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("yaml") | Some("yml")
                    )
                })
                .collect();
            entries.sort();
            for p in entries {
                let name = p.display().to_string();
                let def = loader::parse_rule_file(&p)?;
                rules.push((name.clone(), def));
            }
        }
        Self::from_parts(&manifest, rules)
    }

    /// Build a package from in-memory YAML rule sources — the same
    /// validation path as `load_dir`, minus the filesystem. `files` is
    /// `(name, yaml)` pairs; `name` plays the role of the file path in error
    /// context. Ids become `<package_name>/<id>`; duplicate ids within the
    /// package are an error. Merge the result over another set (e.g.
    /// `RuleSet::built_in()`) with [`RuleSet::merge`].
    pub fn from_sources(
        package_name: &str,
        files: &[(String, String)],
    ) -> Result<RuleSet, LoadError> {
        loader::validate_package_name("style.yaml", package_name)?;
        let manifest = PackageManifest {
            name: package_name.to_string(),
            version: "0.0.0".to_string(),
            description: None,
        };
        let mut rules: Vec<(String, RuleDef)> = Vec::with_capacity(files.len());
        for (name, yaml) in files {
            let def = parse_rule(name, yaml)?;
            rules.push((name.clone(), def));
        }
        Self::from_parts(&manifest, rules)
    }

    /// Assemble a set from a parsed manifest + parsed rule files. Duplicate
    /// ids within the package are an error.
    pub(crate) fn from_parts(
        manifest: &PackageManifest,
        rules: Vec<(String, RuleDef)>,
    ) -> Result<RuleSet, LoadError> {
        let mut set = RuleSet::default();
        for (file, def) in rules {
            let meta = build_meta(&manifest.name, &def);
            if let Some((_, _, first)) = set.defs.iter().find(|(m, _, _)| m.id == meta.id) {
                return Err(LoadError::DuplicateId {
                    id: meta.id.0.clone(),
                    first: first.clone(),
                    second: file,
                });
            }
            set.defs.push((meta, def, file));
        }
        set.rebuild_aliases();
        Ok(set)
    }

    /// Merge another set in; full-id collisions are errors (and leave `self`
    /// unchanged).
    pub fn merge(&mut self, other: RuleSet) -> Result<(), LoadError> {
        for (meta, _, file) in &other.defs {
            if let Some((_, _, first)) = self.defs.iter().find(|(m, _, _)| m.id == meta.id) {
                return Err(LoadError::DuplicateId {
                    id: meta.id.0.clone(),
                    first: first.clone(),
                    second: file.clone(),
                });
            }
        }
        self.defs.extend(other.defs);
        self.rebuild_aliases();
        Ok(())
    }

    /// Recompute the bare-name alias map: a bare name maps to its full id
    /// only when exactly one rule across all packages carries that name.
    fn rebuild_aliases(&mut self) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (meta, _, _) in &self.defs {
            let bare = meta.id.0.rsplit('/').next().unwrap_or(&meta.id.0);
            *counts.entry(bare).or_insert(0) += 1;
        }
        let aliases: HashMap<String, RuleId> = self
            .defs
            .iter()
            .filter_map(|(meta, _, _)| {
                let bare = meta.id.0.rsplit('/').next().unwrap_or(&meta.id.0);
                (counts.get(bare) == Some(&1)).then(|| (bare.to_string(), meta.id.clone()))
            })
            .collect();
        self.aliases = aliases;
    }

    /// Resolve a full id or bare alias to a canonical `RuleId`. Unknown or
    /// ambiguous names resolve to nothing.
    pub fn resolve(&self, id_or_alias: &str) -> Option<&RuleId> {
        if let Some((meta, _, _)) = self.defs.iter().find(|(m, _, _)| m.id.0 == id_or_alias) {
            return Some(&meta.id);
        }
        self.aliases.get(id_or_alias)
    }

    /// True when `name` (full id or alias) resolves to `full_id`.
    fn names_rule(&self, name: &str, full_id: &RuleId) -> bool {
        self.resolve(name) == Some(full_id)
    }

    /// Instantiate fresh rule instances for one run, applying
    /// enable/disable/severity from `options`. `enable` is an allowlist when
    /// present; `disable` wins over `enable`; both accept full ids or bare
    /// aliases (unknown names are silently ignored).
    pub fn instantiate(&self, options: &LintOptions) -> Vec<Box<dyn Rule>> {
        let mut out: Vec<Box<dyn Rule>> = Vec::new();
        for (meta, def, file) in &self.defs {
            if let Some(disable) = &options.disable {
                if disable.iter().any(|d| self.names_rule(d, &meta.id)) {
                    continue;
                }
            }
            if let Some(enable) = &options.enable {
                if !enable.iter().any(|e| self.names_rule(e, &meta.id)) {
                    continue;
                }
            }
            let mut meta = meta.clone();
            if let Some(overrides) = &options.severity {
                for (name, severity) in overrides {
                    if self.names_rule(name, &meta.id) {
                        meta.severity = *severity;
                    }
                }
            }
            let rule: Box<dyn Rule> = match def.engine.as_str() {
                "phrase" => Box::new(
                    PhraseEngine::from_def(meta, def, file)
                        .unwrap_or_else(|e| panic!("{file}: validated rule failed to build: {e}")),
                ),
                "leading" => Box::new(
                    LeadingEngine::from_def(meta, def, file)
                        .unwrap_or_else(|e| panic!("{file}: validated rule failed to build: {e}")),
                ),
                "density" => Box::new(
                    DensityEngine::from_def(meta, def, file)
                        .unwrap_or_else(|e| panic!("{file}: validated rule failed to build: {e}")),
                ),
                "statistical" => Box::new(
                    StatisticalEngine::from_def(meta, def, file)
                        .unwrap_or_else(|e| panic!("{file}: validated rule failed to build: {e}")),
                ),
                "inferential" => Box::new(InferentialRule::from_def(meta, def)),
                other => unreachable!("loader validated engine, got {other:?}"),
            };
            out.push(rule);
        }
        out
    }

    /// Metadata for every rule (for `rules` listings, wasm
    /// `builtInRulesMeta`).
    pub fn metas(&self) -> Vec<&RuleMeta> {
        self.defs.iter().map(|(meta, _, _)| meta).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tier;

    #[test]
    fn builtin_dir_is_embedded_with_manifest() {
        // The embedded package must at least carry its manifest; rules/*.yaml
        // arrive with agent E.
        assert!(BUILTIN_DIR.get_file("style.yaml").is_some());
    }

    #[test]
    fn rule_set_default_is_empty() {
        let rs = RuleSet::default();
        assert!(rs.defs.is_empty());
        assert!(rs.aliases.is_empty());
    }

    // ---- helpers --------------------------------------------------------

    fn manifest(name: &str) -> PackageManifest {
        parse_manifest("style.yaml", &format!("name: {name}\nversion: 0.1.0\n")).unwrap()
    }

    fn rule(file: &str, yaml: &str) -> (String, RuleDef) {
        (file.to_string(), parse_rule(file, yaml).unwrap())
    }

    fn phrase_yaml(id: &str) -> String {
        format!("id: {id}\nengine: phrase\nseverity: error\npatterns: [\"—\"]\n")
    }

    fn inferential_yaml(id: &str) -> String {
        format!(
            "id: {id}\nengine: inferential\nseverity: warning\ngranularity: paragraph\n\
             rubric: Flag it.\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n"
        )
    }

    fn set_of(pkg: &str, rules: Vec<(String, RuleDef)>) -> RuleSet {
        RuleSet::from_parts(&manifest(pkg), rules).unwrap()
    }

    // ---- built_in / load_dir --------------------------------------------

    #[test]
    fn built_in_loads_and_ids_are_namespaced() {
        // Tolerates builtin/rules/ being partially populated during the
        // rewrite; whatever is present must validate.
        let rs = RuleSet::built_in();
        for meta in rs.metas() {
            assert!(
                meta.id.0.starts_with("core/"),
                "unexpected id {}",
                meta.id.0
            );
        }
    }

    #[test]
    fn load_dir_reads_manifest_and_rules() {
        let base =
            std::env::temp_dir().join(format!("lawlint-registry-load-dir-{}", std::process::id()));
        let rules_dir = base.join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        std::fs::write(rules_dir.join("no-x.yaml"), phrase_yaml("no-x")).unwrap();
        std::fs::create_dir_all(rules_dir.join("skills")).unwrap();
        std::fs::write(
            rules_dir.join("soft-check.yaml"),
            "id: soft-check\nengine: inferential\nskill: ./skills/soft-check.md\n",
        )
        .unwrap();
        std::fs::write(
            rules_dir.join("skills/soft-check.md"),
            "---\ndescription: Soft check.\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n---\nFlag this pattern.\n",
        )
        .unwrap();
        std::fs::write(rules_dir.join("notes.txt"), "not a rule").unwrap();

        let rs = RuleSet::load_dir(&base).unwrap();
        assert_eq!(rs.metas().len(), 2);
        assert!(rs.resolve("no-x").is_some());
        assert_eq!(rs.metas()[1].id.0, "firm/soft-check");
        assert_eq!(rs.resolve("no-x").unwrap().0, "firm/no-x");

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn load_dir_rejects_skill_on_phrase_rule() {
        let base =
            std::env::temp_dir().join(format!("lawlint-registry-collision-{}", std::process::id()));
        let rules_dir = base.join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        std::fs::write(
            rules_dir.join("same.yaml"),
            "id: same\nengine: phrase\nskill: ./same.md\npatterns: [x]\n",
        )
        .unwrap();
        std::fs::write(
            rules_dir.join("same.md"),
            "---\nflag_examples: [a, b, c]\npass_examples: [x, y, z]\n---\nFlag it.\n",
        )
        .unwrap();

        let e = RuleSet::load_dir(&base).unwrap_err();
        assert!(e.to_string().contains("same.yaml: skill"), "{e}");
        assert!(e.to_string().contains("phrase"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn load_dir_reports_missing_skill_file() {
        let base = std::env::temp_dir().join(format!(
            "lawlint-registry-missing-skill-{}",
            std::process::id()
        ));
        let rules_dir = base.join("rules");
        std::fs::create_dir_all(&rules_dir).unwrap();
        std::fs::write(base.join("style.yaml"), "name: firm\nversion: 1.0.0\n").unwrap();
        std::fs::write(
            rules_dir.join("soft.yaml"),
            "id: soft\nengine: inferential\nskill: ./missing.md\n",
        )
        .unwrap();

        let e = RuleSet::load_dir(&base).unwrap_err();
        assert!(e.to_string().contains("missing.md"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn load_dir_missing_manifest_is_io_error() {
        let base =
            std::env::temp_dir().join(format!("lawlint-registry-missing-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let e = RuleSet::load_dir(&base).unwrap_err();
        assert!(matches!(e, LoadError::Io { .. }), "{e}");
        assert!(e.to_string().contains("style.yaml"), "{e}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    // ---- from_sources ---------------------------------------------------

    #[test]
    fn from_sources_matches_load_dir_semantics() {
        let rs = RuleSet::from_sources("user", &[("no-x.yaml".to_string(), phrase_yaml("no-x"))])
            .unwrap();
        assert_eq!(rs.metas().len(), 1);
        assert_eq!(rs.metas()[0].id.0, "user/no-x");
        assert_eq!(rs.resolve("no-x").unwrap().0, "user/no-x");
        // Merges over the built-ins like any loaded package.
        let mut merged = RuleSet::built_in();
        merged.merge(rs).unwrap();
        assert!(merged.resolve("user/no-x").is_some());
        assert!(merged.resolve("core/no-em-dash-overuse").is_some());
    }

    #[test]
    fn from_sources_validates_package_name_rules_and_duplicates() {
        let e = RuleSet::from_sources("bad name", &[]).unwrap_err();
        assert!(e.to_string().contains("package name"), "{e}");

        let e = RuleSet::from_sources(
            "user",
            &[(
                "bad.yaml".to_string(),
                "id: no-x\nengine: phrase\nseverity: high\npatterns: [x]\n".to_string(),
            )],
        )
        .unwrap_err();
        assert_eq!(e.file(), "bad.yaml");
        assert!(e.to_string().contains("severity"), "{e}");

        let e = RuleSet::from_sources(
            "user",
            &[
                ("a.yaml".to_string(), phrase_yaml("no-x")),
                ("b.yaml".to_string(), phrase_yaml("no-x")),
            ],
        )
        .unwrap_err();
        assert_eq!(e.file(), "b.yaml");
        assert!(
            e.to_string().contains("duplicate rule id \"user/no-x\""),
            "{e}"
        );
    }

    #[test]
    fn duplicate_id_within_package_errors() {
        let e = RuleSet::from_parts(
            &manifest("core"),
            vec![
                rule("a.yaml", &phrase_yaml("no-x")),
                rule("b.yaml", &phrase_yaml("no-x")),
            ],
        )
        .unwrap_err();
        let s = e.to_string();
        assert!(s.contains("duplicate rule id \"core/no-x\""), "{s}");
        assert!(s.contains("a.yaml") && s.contains("b.yaml"), "{s}");
    }

    // ---- meta derivation ------------------------------------------------

    #[test]
    fn meta_defaults_scope_severity_and_docs_url() {
        let rs = set_of(
            "core",
            vec![rule(
                "r.yaml",
                "id: no-x\nengine: phrase\npatterns: [\"—\"]\n",
            )],
        );
        let meta = rs.metas()[0];
        assert_eq!(meta.scope, Scope::Text);
        assert_eq!(meta.severity, Severity::Warning);
        assert_eq!(meta.docs_url, "https://lawlint.com/rules/no-x");
        assert_eq!(meta.tier, Tier::Static);
    }

    #[test]
    fn meta_respects_explicit_fields_and_tier_derivation() {
        let rs = set_of(
            "core",
            vec![
                rule(
                    "d.yaml",
                    "id: dens\nengine: density\nscope: prose\nseverity: suggestion\n\
                     docs: \"https://example.com/dens\"\nthreshold: 8\npatterns: [\"x\"]\n",
                ),
                rule(
                    "s.yaml",
                    "id: stat\nengine: statistical\nmetric: sentence-length\n",
                ),
                rule("i.yaml", &inferential_yaml("inf")),
            ],
        );
        let metas = rs.metas();
        let by_id = |id: &str| *metas.iter().find(|m| m.id.0 == id).unwrap();
        let d = by_id("core/dens");
        assert_eq!(d.scope, Scope::Prose);
        assert_eq!(d.severity, Severity::Suggestion);
        assert_eq!(d.docs_url, "https://example.com/dens");
        assert_eq!(d.tier, Tier::Statistical);
        assert_eq!(by_id("core/stat").tier, Tier::Statistical);
        assert_eq!(by_id("core/inf").tier, Tier::Inferential);
    }

    #[test]
    fn meta_intent_defaults_detection_and_respects_style() {
        let rs = set_of(
            "core",
            vec![
                rule("a.yaml", "id: no-x\nengine: phrase\npatterns: [\"—\"]\n"),
                rule(
                    "b.yaml",
                    "id: no-y\nengine: phrase\nintent: style\npatterns: [\";\"]\n",
                ),
            ],
        );
        let metas = rs.metas();
        let by_id = |id: &str| *metas.iter().find(|m| m.id.0 == id).unwrap();
        assert_eq!(by_id("core/no-x").intent, Intent::Detection);
        assert_eq!(by_id("core/no-y").intent, Intent::Style);
    }

    // ---- resolve / aliases ----------------------------------------------

    #[test]
    fn resolve_full_id_and_alias() {
        let rs = set_of("core", vec![rule("r.yaml", &phrase_yaml("no-x"))]);
        assert_eq!(rs.resolve("core/no-x").unwrap().0, "core/no-x");
        assert_eq!(rs.resolve("no-x").unwrap().0, "core/no-x");
        assert!(rs.resolve("no-y").is_none());
        assert!(rs.resolve("other/no-x").is_none());
    }

    #[test]
    fn ambiguous_alias_resolves_to_nothing() {
        let mut rs = set_of("core", vec![rule("a.yaml", &phrase_yaml("no-x"))]);
        rs.merge(set_of("firm", vec![rule("b.yaml", &phrase_yaml("no-x"))]))
            .unwrap();
        // Bare name now ambiguous: silently prefers nothing.
        assert!(rs.resolve("no-x").is_none());
        // Full ids still resolve.
        assert_eq!(rs.resolve("core/no-x").unwrap().0, "core/no-x");
        assert_eq!(rs.resolve("firm/no-x").unwrap().0, "firm/no-x");
    }

    #[test]
    fn merge_collision_errors_and_leaves_self_unchanged() {
        let mut rs = set_of("core", vec![rule("a.yaml", &phrase_yaml("no-x"))]);
        let e = rs
            .merge(set_of("core", vec![rule("b.yaml", &phrase_yaml("no-x"))]))
            .unwrap_err();
        assert!(e.to_string().contains("duplicate rule id \"core/no-x\""));
        assert_eq!(rs.metas().len(), 1);
        assert_eq!(rs.resolve("no-x").unwrap().0, "core/no-x");
    }

    #[test]
    fn merge_extends_defs_and_aliases() {
        let mut rs = set_of("core", vec![rule("a.yaml", &phrase_yaml("no-x"))]);
        rs.merge(set_of("firm", vec![rule("b.yaml", &phrase_yaml("no-y"))]))
            .unwrap();
        assert_eq!(rs.metas().len(), 2);
        assert_eq!(rs.resolve("no-y").unwrap().0, "firm/no-y");
    }

    // ---- instantiate ----------------------------------------------------
    //
    // Filtering/override tests use inferential rules only: InferentialRule
    // lives here and has no dependency on the engine constructors owned by
    // agents B/C. The per-engine happy path is covered separately below.

    #[test]
    fn instantiate_applies_disable_by_alias_and_full_id() {
        let rs = set_of(
            "core",
            vec![
                rule("a.yaml", &inferential_yaml("inf-a")),
                rule("b.yaml", &inferential_yaml("inf-b")),
            ],
        );
        let opts = LintOptions {
            disable: Some(vec!["inf-a".into()]),
            ..Default::default()
        };
        let rules = rs.instantiate(&opts);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].meta().id.0, "core/inf-b");

        let opts = LintOptions {
            disable: Some(vec!["core/inf-b".into()]),
            ..Default::default()
        };
        let rules = rs.instantiate(&opts);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].meta().id.0, "core/inf-a");
    }

    #[test]
    fn instantiate_enable_is_an_allowlist_and_disable_wins() {
        let rs = set_of(
            "core",
            vec![
                rule("a.yaml", &inferential_yaml("inf-a")),
                rule("b.yaml", &inferential_yaml("inf-b")),
            ],
        );
        let opts = LintOptions {
            enable: Some(vec!["inf-a".into()]),
            ..Default::default()
        };
        let rules = rs.instantiate(&opts);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].meta().id.0, "core/inf-a");

        let opts = LintOptions {
            enable: Some(vec!["inf-a".into()]),
            disable: Some(vec!["inf-a".into()]),
            ..Default::default()
        };
        assert!(rs.instantiate(&opts).is_empty());
    }

    #[test]
    fn instantiate_unknown_names_are_silently_ignored() {
        let rs = set_of("core", vec![rule("a.yaml", &inferential_yaml("inf-a"))]);
        let opts = LintOptions {
            disable: Some(vec!["no-such-rule".into()]),
            ..Default::default()
        };
        assert_eq!(rs.instantiate(&opts).len(), 1);
    }

    #[test]
    fn instantiate_applies_severity_override_via_alias() {
        let rs = set_of("core", vec![rule("a.yaml", &inferential_yaml("inf-a"))]);
        let opts = LintOptions {
            severity: Some(
                [("inf-a".to_string(), Severity::Suggestion)]
                    .into_iter()
                    .collect(),
            ),
            ..Default::default()
        };
        let rules = rs.instantiate(&opts);
        assert_eq!(rules[0].meta().severity, Severity::Suggestion);
        // Override reaches the rubric fragment too.
        assert_eq!(rules[0].rubric().unwrap().severity, Severity::Suggestion);
        // Fresh default run is untouched (fresh instances per run).
        let rules = rs.instantiate(&LintOptions::default());
        assert_eq!(rules[0].meta().severity, Severity::Warning);
    }

    #[test]
    fn inferential_rule_carries_rubric_fragment() {
        let rs = set_of("core", vec![rule("a.yaml", &inferential_yaml("inf-a"))]);
        let rules = rs.instantiate(&LintOptions::default());
        let fragment = rules[0].rubric().expect("inferential rule has a rubric");
        assert_eq!(fragment.rule.0, "core/inf-a");
        assert_eq!(fragment.granularity, Granularity::Paragraph);
        assert_eq!(fragment.rubric, "Flag it.");
        assert_eq!(fragment.flag_examples.len(), 3);
        assert_eq!(fragment.pass_examples.len(), 3);
        let interests = rules[0].interests();
        assert!(
            !interests.tokens
                && !interests.sentences
                && !interests.blocks
                && !interests.document_exit
        );
    }

    #[test]
    fn instantiate_happy_path_per_engine_kind() {
        // Exercises every engine constructor; depends on agents B/C engine
        // implementations landing.
        let rs = set_of(
            "core",
            vec![
                rule("p.yaml", &phrase_yaml("no-p")),
                rule(
                    "l.yaml",
                    "id: no-l\nengine: leading\npatterns: [\"Certainly\"]\n",
                ),
                rule(
                    "d.yaml",
                    "id: no-d\nengine: density\nthreshold: 8\npatterns: [\"x\"]\n",
                ),
                rule(
                    "s.yaml",
                    "id: no-s\nengine: statistical\nmetric: repetitive-openers\n\
                     params: { run_length: 3 }\n",
                ),
                rule("i.yaml", &inferential_yaml("no-i")),
            ],
        );
        let rules = rs.instantiate(&LintOptions::default());
        assert_eq!(rules.len(), 5);
        let ids: Vec<_> = rules.iter().map(|r| r.meta().id.0.clone()).collect();
        assert_eq!(
            ids,
            vec![
                "core/no-p",
                "core/no-l",
                "core/no-d",
                "core/no-s",
                "core/no-i"
            ]
        );
        // Tier derived from engine, visible on the instantiated rule.
        assert_eq!(rules[0].meta().tier, Tier::Static);
        assert_eq!(rules[2].meta().tier, Tier::Statistical);
        assert_eq!(rules[4].meta().tier, Tier::Inferential);
    }
}

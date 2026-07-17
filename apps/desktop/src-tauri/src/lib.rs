use std::path::Path;

use lawlint_core::{lint as lint_text, lint_with, LintOptions, LintResult, RuleSet};

/// Command body, separated from the `#[tauri::command]` wrapper for testing.
///
/// `options.ruleDirs` (consumed here, ignored by core `lint()`) merges extra
/// YAML rule packages on top of the built-ins. Load/merge failures reject the
/// invoke promise with a plain-English message; the success JSON shape is
/// unchanged (a bare `LintResult`).
fn run_lint(text: &str, options: Option<LintOptions>) -> Result<LintResult, String> {
    let options = options.unwrap_or_default();
    let rule_dirs = options.rule_dirs.as_deref().unwrap_or_default();
    if rule_dirs.is_empty() {
        return Ok(lint_text(text, &options));
    }
    let mut rules = RuleSet::built_in();
    for dir in rule_dirs {
        let extra = RuleSet::load_dir(Path::new(dir)).map_err(|e| e.to_string())?;
        rules.merge(extra).map_err(|e| e.to_string())?;
    }
    Ok(lint_with(text, &options, &rules))
}

#[tauri::command]
fn lint(text: String, options: Option<LintOptions>) -> Result<LintResult, String> {
    run_lint(&text, options)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![lint])
        .run(tauri::generate_context!())
        .expect("error while running lawlint desktop application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lints_with_built_ins_when_no_options() {
        let result = run_lint("We delve into it.", None).unwrap();
        assert!(!result.diagnostics.is_empty());
        // v2 ids are namespaced.
        assert!(result
            .diagnostics
            .iter()
            .any(|d| d.rule_id.0 == "core/no-ai-cliches"));
        assert!(result.diagnostics.iter().all(|d| d.rule_id.0.contains('/')));
    }

    #[test]
    fn json_shape_matches_frontend_contract() {
        // The frontend reads ruleId / severity / message / line / column /
        // stats.{wordCount,sentenceCount,score}; severity "info" is now
        // "suggestion" and diagnostics carry span/tier (+ confidence for
        // tier 3).
        let result = run_lint("The order is null and void.", None).unwrap();
        let json = serde_json::to_value(&result).unwrap();
        let d = &json["diagnostics"][0];
        assert_eq!(d["ruleId"], "core/no-doublets");
        assert_eq!(d["severity"], "suggestion");
        assert!(d["message"].is_string());
        assert_eq!(d["line"], 1);
        assert!(d["column"].is_number());
        assert!(d["span"]["start"].is_number());
        assert_eq!(d["tier"], "static");
        assert!(json["stats"]["wordCount"].is_number());
        assert!(json["stats"]["sentenceCount"].is_number());
        assert!(json["stats"]["score"].is_number());
    }

    #[test]
    fn options_pass_through() {
        let options: LintOptions = serde_json::from_str(
            r#"{"markdown": true, "disable": ["no-ai-cliches", "no-marketing-language"]}"#,
        )
        .unwrap();
        let result = run_lint(
            "We delve.\n\n```\npursuant to — herein\n```\n",
            Some(options),
        )
        .unwrap();
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
    }

    #[test]
    fn rule_dirs_merge_extra_packages() {
        let dir = std::env::temp_dir().join("lawlint-desktop-test-rules");
        let rules = dir.join("rules");
        std::fs::create_dir_all(&rules).unwrap();
        std::fs::write(dir.join("style.yaml"), "name: firm\nversion: \"1.0\"\n").unwrap();
        std::fs::write(
            rules.join("no-foo.yaml"),
            concat!(
                "id: no-foo\n",
                "engine: phrase\n",
                "severity: error\n",
                "description: \"No foo.\"\n",
                "message: \"Avoid foo\"\n",
                "patterns:\n",
                "  - \"(?i)\\\\bfoo\\\\b\"\n",
            ),
        )
        .unwrap();

        let options: LintOptions =
            serde_json::from_str(&format!(r#"{{"ruleDirs": [{:?}]}}"#, dir.to_string_lossy()))
                .unwrap();
        let result = run_lint("We delve into foo.", Some(options)).unwrap();
        let ids: Vec<&str> = result
            .diagnostics
            .iter()
            .map(|d| d.rule_id.0.as_str())
            .collect();
        assert!(ids.contains(&"firm/no-foo"), "{ids:?}");
        assert!(ids.contains(&"core/no-ai-cliches"), "{ids:?}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bad_rule_dir_is_a_plain_error() {
        let options: LintOptions =
            serde_json::from_str(r#"{"ruleDirs": ["/nonexistent/lawlint-rules"]}"#).unwrap();
        let err = run_lint("text", Some(options)).unwrap_err();
        assert!(!err.is_empty());
    }
}

//! CLI integration tests (assert_cmd). Every command runs with cwd set to a
//! fresh temp dir so config discovery never escapes into the repo.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::TempDir;

fn cmd(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("lawlint").unwrap();
    cmd.current_dir(dir.path());
    cmd
}

fn write(dir: &TempDir, rel: &str, content: &str) -> std::path::PathBuf {
    let path = dir.path().join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    path
}

/// A user rule package with one MachineApplicable-fixable phrase rule.
fn write_fix_package(dir: &TempDir) -> std::path::PathBuf {
    write(dir, "pkg/style.yaml", "name: firm\nversion: 1.0.0\n");
    write(
        dir,
        "pkg/rules/use-plain.yaml",
        concat!(
            "id: use-plain\n",
            "engine: phrase\n",
            "severity: error\n",
            "description: Prefer plain words.\n",
            "patterns:\n",
            "  - { pattern: \"\\\\butilize\\\\b\", message: \"Prefer use\", suggestion: \"use\", fix: \"use\" }\n",
        ),
    );
    dir.path().join("pkg")
}

// ---- lint: stdin, formats, exit codes ----------------------------------

#[test]
fn lint_stdin_pretty_reports_findings_and_score() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .write_stdin("We map the landscape of this matter.")
        .assert()
        .code(0) // warnings only, no limit
        .stdout(predicate::str::contains("core/no-ai-cliches"))
        .stdout(predicate::str::contains("Human-likeness score:"));
}

#[test]
fn lint_stdin_json_has_contract_field_names() {
    let dir = TempDir::new().unwrap();
    let output = cmd(&dir)
        .args(["--format", "json"])
        .write_stdin("We map the landscape of it.")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["diagnostics"][0]["ruleId"], "core/no-ai-cliches");
    assert_eq!(json["diagnostics"][0]["severity"], "warning");
    assert_eq!(json["stats"]["wordCount"], 6);
    assert!(json["stats"]["score"].is_i64());
    assert!(json.get("judge").is_none());
}

#[test]
fn lint_exit_codes() {
    let dir = TempDir::new().unwrap();
    // Clean prose → 0.
    cmd(&dir)
        .write_stdin("The court granted the motion.")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("No issues found"));
    // Error-severity finding (sycophantic opener) → 1.
    cmd(&dir)
        .write_stdin("Great question! The answer is no.")
        .assert()
        .code(1);
    // Warnings over --max-warnings → 1.
    cmd(&dir)
        .args(["--max-warnings", "0"])
        .write_stdin("We map the landscape of it.")
        .assert()
        .code(1);
    // Same input under the default (inf) limit → 0.
    cmd(&dir)
        .write_stdin("We map the landscape of it.")
        .assert()
        .code(0);
    // Missing file → 2.
    cmd(&dir)
        .arg("no-such-file.txt")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("failed to read"));
}

#[test]
fn quiet_suppresses_pretty_output_but_keeps_exit_code() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .arg("--quiet")
        .write_stdin("Great question! The answer is no.")
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty());
}

#[test]
fn disable_flag_silences_a_rule_by_flat_alias() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["--disable", "no-ai-cliches"])
        .write_stdin("We map the landscape of it.")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no-ai-cliches").not());
}

// ---- config discovery --------------------------------------------------

#[test]
fn config_file_is_discovered_and_applied() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "lawlint.config.json",
        r#"{"disable": ["no-ai-cliches"]}"#,
    );
    cmd(&dir)
        .write_stdin("We map the landscape of it.")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no-ai-cliches").not());
}

#[test]
fn invalid_config_is_a_config_error_exit_2() {
    let dir = TempDir::new().unwrap();
    write(&dir, "lawlint.config.json", "{not json");
    cmd(&dir)
        .write_stdin("Anything.")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("invalid config"));
}

#[test]
fn config_rule_dirs_load_relative_to_config() {
    let dir = TempDir::new().unwrap();
    write_fix_package(&dir);
    write(&dir, "lawlint.config.json", r#"{"ruleDirs": ["pkg"]}"#);
    // Run from a nested cwd; the config's ruleDirs still resolve.
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    let mut cmd = Command::cargo_bin("lawlint").unwrap();
    cmd.current_dir(dir.path().join("sub"))
        .write_stdin("We utilize tools.")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("firm/use-plain"));
}

#[test]
fn nested_config_is_discovered_and_wins_over_legacy() {
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        ".lawlint/config.json",
        r#"{"disable": ["no-ai-cliches"]}"#,
    );
    write(&dir, "lawlint.config.json", "{}");
    cmd(&dir)
        .write_stdin("We map the landscape of it.")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no-ai-cliches").not())
        .stderr(predicate::str::contains("both"));
}

#[test]
fn nested_config_rule_dirs_resolve_from_project_root() {
    let dir = TempDir::new().unwrap();
    write_fix_package(&dir);
    write(&dir, ".lawlint/config.json", r#"{"ruleDirs": ["pkg"]}"#);
    fs::create_dir_all(dir.path().join("sub")).unwrap();
    let mut cmd = Command::cargo_bin("lawlint").unwrap();
    cmd.current_dir(dir.path().join("sub"))
        .write_stdin("We utilize tools.")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("firm/use-plain"));
}

// ---- init --------------------------------------------------------------

#[test]
fn init_yes_writes_config_and_refuses_to_overwrite() {
    let dir = TempDir::new().unwrap();
    cmd(&dir).args(["init", "--yes"]).assert().code(0);
    let config = fs::read_to_string(dir.path().join(".lawlint/config.json")).unwrap();
    assert!(config.contains("\"enabled\": false"));
    cmd(&dir)
        .args(["init", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--force"));
    cmd(&dir)
        .args(["init", "--yes", "--force"])
        .assert()
        .code(0);
}

#[test]
fn init_ai_flag_writes_preference_without_prompting_or_downloading() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["init", "--yes", "--ai", "gemma"])
        .assert()
        .code(0);
    let config = fs::read_to_string(dir.path().join(".lawlint/config.json")).unwrap();
    assert!(config.contains("\"model\": \"local:google/gemma-4-E4B-it-qat-q4_0-gguf\""));
    // Invalid values are a config error (exit 2) with guidance.
    cmd(&dir)
        .args(["init", "--yes", "--force", "--ai", "gpt4"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("qwen, gemma"));
}

#[test]
fn init_scaffolds_a_working_rules_package() {
    let dir = TempDir::new().unwrap();
    // Prompts: AI model (default: local Qwen), judge (default: disabled),
    // markdown (default: no), starter rules package (yes).
    cmd(&dir)
        .arg("init")
        .write_stdin("\n\n\ny\n")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(".lawlint/rules/style.yaml"));
    // The scaffolded package is discovered via the written ruleDirs…
    cmd(&dir)
        .arg("rules")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no-avoidance-of-doubt"));
    // …its own examples pass, and it fires on real input.
    cmd(&dir)
        .args(["rules", "test", ".lawlint/rules"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("0 failed"));
    cmd(&dir)
        .write_stdin("For the avoidance of doubt, the fee is due monthly.")
        .assert()
        .stdout(predicate::str::contains("no-avoidance-of-doubt"));
}

#[test]
fn bad_rule_dir_prints_load_error_verbatim_and_exits_2() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(dir.path().join("empty-pkg")).unwrap();
    cmd(&dir)
        .args(["--rule-dir", "empty-pkg"])
        .write_stdin("Anything.")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("style.yaml"));
}

// ---- markdown ----------------------------------------------------------

#[test]
fn markdown_auto_enables_for_md_files() {
    let dir = TempDir::new().unwrap();
    // Code block content must not be linted when markdown is on.
    let path = write(
        &dir,
        "doc.md",
        "Clean prose here.\n\n```\nwe delve — daily\n```\n",
    );
    cmd(&dir)
        .arg(path.to_str().unwrap())
        .assert()
        .code(0)
        .stdout(predicate::str::contains("No issues found"));
}

// ---- --fix -------------------------------------------------------------

#[test]
fn fix_applies_machine_applicable_fixes_in_place() {
    let dir = TempDir::new().unwrap();
    let pkg = write_fix_package(&dir);
    let file = write(&dir, "brief.txt", "We utilize tools.");
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .args(["--rule-dir", pkg.to_str().unwrap(), "--fix"])
        .assert()
        .code(1) // the error-severity finding still counts
        .stderr(predicate::str::contains("Applied 1 fix")); // status → stderr
    assert_eq!(fs::read_to_string(&file).unwrap(), "We use tools.");
}

#[test]
fn fix_with_json_format_keeps_stdout_machine_parseable() {
    // Regression: the "Applied N fix(es)" status line must go to stderr, or
    // `--format json --fix | jq` gets `{...}\nApplied 1 fix...` on stdout.
    let dir = TempDir::new().unwrap();
    let pkg = write_fix_package(&dir);
    let file = write(&dir, "brief.txt", "We utilize tools.");
    let output = cmd(&dir)
        .arg(file.to_str().unwrap())
        .args([
            "--rule-dir",
            pkg.to_str().unwrap(),
            "--format",
            "json",
            "--fix",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("stdout must be pure JSON when --format json is combined with --fix");
    assert_eq!(json["diagnostics"][0]["ruleId"], "firm/use-plain");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Applied 1 fix"),
        "status line moved to stderr: {stderr}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "We use tools.");
}

#[test]
fn fix_requires_a_file_argument() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .arg("--fix")
        .write_stdin("We utilize tools.")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--fix requires a FILE"));
}

#[test]
fn fix_without_applicable_fixes_leaves_file_alone() {
    let dir = TempDir::new().unwrap();
    let file = write(&dir, "brief.txt", "We map the landscape of it.");
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .arg("--fix")
        .assert()
        .code(0)
        .stderr(predicate::str::contains("Applied 0 fixes"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "We map the landscape of it."
    );
}

// ---- .docx -------------------------------------------------------------

const DOCX_FIXTURE: &[u8] = include_bytes!("../../lawlint-docx/tests/fixtures/sample.docx");

#[test]
fn docx_is_linted_like_text() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("brief.docx");
    fs::write(&file, DOCX_FIXTURE).unwrap();
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .assert()
        .code(0) // findings are warnings; no error severity
        .stdout(predicate::str::contains("no-legalese"));
}

#[test]
fn docx_fix_writes_tracked_changes_not_plain_text() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("brief.docx");
    fs::write(&file, DOCX_FIXTURE).unwrap();
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .arg("--fix")
        .assert()
        .code(0)
        .stderr(predicate::str::contains("tracked change"));
    // Still a valid zip (docx), and modified from the original.
    let after = fs::read(&file).unwrap();
    assert_eq!(&after[..2], b"PK", "output is still a .docx (zip)");
    assert_ne!(
        after, DOCX_FIXTURE,
        "the fix should have rewritten the file"
    );
}

// ---- --diff ------------------------------------------------------------

#[test]
fn diff_previews_without_modifying_the_file() {
    let dir = TempDir::new().unwrap();
    let file = write(
        &dir,
        "brief.txt",
        "Pursuant to Section 4(b), the parties proceed.",
    );
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .arg("--diff")
        .assert()
        .stdout(predicate::str::contains("- Pursuant to Section 4(b),"))
        .stdout(predicate::str::contains("+ Under Section 4(b),"));
    // Preview only: the file is untouched.
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "Pursuant to Section 4(b), the parties proceed."
    );
}

#[test]
fn diff_renders_context_lines_and_gap_separators() {
    let dir = TempDir::new().unwrap();
    // Two fixable findings separated by enough clean lines that the
    // unchanged run between them collapses into a separator.
    let file = write(
        &dir,
        "brief.txt",
        "Pursuant to Section 4(b), rent is due.\n\
         The lease term is five years.\n\
         The premises are in good repair.\n\
         The deposit is held in escrow.\n\
         The landlord waives none of its rights.\n\
         Fees accrue pursuant to the rider.",
    );
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .arg("--diff")
        .assert()
        .stdout(predicate::str::contains("- Pursuant to Section 4(b),"))
        .stdout(predicate::str::contains("+ Under Section 4(b),"))
        .stdout(predicate::str::contains("- Fees accrue pursuant to"))
        .stdout(predicate::str::contains("+ Fees accrue under"))
        // One unchanged context line around each hunk, dim-rendered with a
        // two-space prefix, and the collapsed run between hunks as "···".
        .stdout(predicate::str::contains("  The lease term is five years."))
        .stdout(predicate::str::contains("···"));
}

#[test]
fn prompt_format_with_quiet_suppresses_stdout_but_keeps_exit_code() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["-", "--format", "prompt", "--quiet"])
        .write_stdin("It was—wrong.")
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty());
}

#[test]
fn diff_with_json_format_is_a_config_error() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["--diff", "--format", "json"])
        .write_stdin("Pursuant to Section 4(b), the parties proceed.")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--diff requires --format pretty"));
}

#[test]
fn diff_reports_no_applicable_fixes() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .arg("--diff")
        .write_stdin("The court granted the motion.")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no applicable fixes"));
}

#[test]
fn fix_and_diff_rewrites_file_and_prints_markers() {
    let dir = TempDir::new().unwrap();
    let file = write(
        &dir,
        "brief.txt",
        "Pursuant to Section 4(b), the parties proceed.",
    );
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .args(["--fix", "--diff"])
        .assert()
        .stdout(predicate::str::contains("- Pursuant to Section 4(b),"))
        .stdout(predicate::str::contains("+ Under Section 4(b),"))
        .stderr(predicate::str::contains("Applied 1 fix"));
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "Under Section 4(b), the parties proceed."
    );
}

// ---- --format prompt ---------------------------------------------------

#[test]
fn prompt_format_emits_revision_brief_with_section_and_excerpt() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["--format", "prompt"])
        .write_stdin("Great question! The answer is no.")
        .assert()
        .code(1) // error-severity finding drives the exit code, not the format
        .stdout(predicate::str::contains("## core/no-sycophantic-openers"))
        .stdout(predicate::str::contains("The answer is no."))
        // Stdin input is embedded so the brief is self-contained.
        .stdout(predicate::str::contains("Document to revise:"));
}

#[test]
fn prompt_format_file_input_references_path_without_embedding() {
    let dir = TempDir::new().unwrap();
    let file = write(
        &dir,
        "brief.txt",
        "Pursuant to Section 4(b), rent is due.\nThe lease term is five years.",
    );
    let path = file.to_str().unwrap();
    cmd(&dir)
        .arg(path)
        .args(["--format", "prompt"])
        .assert()
        .stdout(predicate::str::contains(format!(
            "Revise the document at `{path}`."
        )))
        .stdout(predicate::str::contains(format!("Edit `{path}` in place")))
        // The document body stays out of the brief — the receiving agent
        // reads the file itself. The un-flagged line must not appear.
        .stdout(predicate::str::contains("Document to revise:").not())
        .stdout(predicate::str::contains("The lease term is five years.").not());
}

#[test]
fn prompt_format_clean_text_prints_nothing_but_notes_on_stderr() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["--format", "prompt"])
        .write_stdin("The court granted the motion.")
        .assert()
        .code(0)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "no issues found; no prompt generated",
        ));
}

#[test]
fn prompt_format_with_fix_is_a_config_error() {
    let dir = TempDir::new().unwrap();
    let file = write(&dir, "brief.txt", "Great question! The answer is no.");
    cmd(&dir)
        .arg(file.to_str().unwrap())
        .args(["--format", "prompt", "--fix"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--fix and --diff require --format pretty",
        ));
}

#[test]
fn prompt_format_with_diff_is_a_config_error() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["--format", "prompt", "--diff"])
        .write_stdin("Great question! The answer is no.")
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "--fix and --diff require --format pretty",
        ));
}

// ---- rules subcommand --------------------------------------------------

#[test]
fn rules_json_lists_all_built_ins_in_website_contract_shape() {
    // Regression: the website's rules pages consume this output as
    // [{id, meta: {description, severity, docsUrl, ...}}] with flat ids that
    // match both the /rules/<id> routes and the docsUrl slugs.
    let dir = TempDir::new().unwrap();
    let output = cmd(&dir).args(["rules", "--json"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let rules = json.as_array().unwrap();
    assert_eq!(rules.len(), 22);
    for rule in rules {
        let id = rule["id"].as_str().unwrap();
        assert!(!id.contains('/'), "ids must be flat, got {id}");
        let meta = rule
            .get("meta")
            .unwrap_or_else(|| panic!("{id}: missing meta wrapper"));
        assert!(meta["description"].is_string());
        assert!(meta["severity"].is_string());
        assert!(meta["tier"].is_string());
        let docs_url = meta["docsUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("{id}: docsUrl"));
        assert!(
            docs_url.ends_with(&format!("/rules/{id}")),
            "{id}: docsUrl slug must match the flat id, got {docs_url}"
        );
        assert!(
            meta.get("docs_url").is_none(),
            "{id}: docs_url must be camelCase"
        );
    }
}

#[test]
fn rules_pretty_lists_ids_and_merges_rule_dirs() {
    let dir = TempDir::new().unwrap();
    let pkg = write_fix_package(&dir);
    cmd(&dir)
        .args(["rules", "--rule-dir", pkg.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("core/no-em-dash"))
        .stdout(predicate::str::contains("firm/use-plain"));
}

// ---- rules test --------------------------------------------------------

#[test]
fn rules_test_passing_rule_exits_0() {
    let dir = TempDir::new().unwrap();
    let rule = write(
        &dir,
        "no-xylophone.yaml",
        concat!(
            "id: no-xylophone\n",
            "engine: phrase\n",
            "severity: error\n",
            "message: No xylophones.\n",
            "patterns: [\"(?i)\\\\bxylophone\\\\b\"]\n",
            "examples:\n",
            "  - { bad: \"A xylophone appears.\", good: \"A piano appears.\" }\n",
        ),
    );
    cmd(&dir)
        .args(["rules", "test", rule.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("test/no-xylophone"))
        .stdout(predicate::str::contains("bad[0]"))
        .stdout(predicate::str::contains("2 passed, 0 failed"));
}

#[test]
fn rules_test_failing_example_exits_1() {
    let dir = TempDir::new().unwrap();
    // Deliberately broken: the bad example never matches the pattern, and
    // the good example does.
    let rule = write(
        &dir,
        "no-xylophone.yaml",
        concat!(
            "id: no-xylophone\n",
            "engine: phrase\n",
            "severity: error\n",
            "patterns: [\"(?i)\\\\bxylophone\\\\b\"]\n",
            "examples:\n",
            "  - { bad: \"A piano appears.\", good: \"A xylophone appears.\" }\n",
        ),
    );
    cmd(&dir)
        .args(["rules", "test", rule.to_str().unwrap()])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("FAIL"))
        .stdout(predicate::str::contains("0 passed, 2 failed"));
}

#[test]
fn rules_test_package_dir_uses_manifest_name_and_invalid_rule_fails() {
    let dir = TempDir::new().unwrap();
    write(&dir, "pkg/style.yaml", "name: firm\nversion: 1.0.0\n");
    write(
        &dir,
        "pkg/rules/ok.yaml",
        concat!(
            "id: ok\n",
            "engine: phrase\n",
            "patterns: [\"zebra\"]\n",
            "examples:\n",
            "  - { bad: \"One zebra.\", good: \"One horse.\" }\n",
        ),
    );
    write(
        &dir,
        "pkg/rules/broken.yaml",
        "id: broken\nengine: phrase\npatterns: [\"(\"]\n",
    );
    cmd(&dir)
        .args(["rules", "test", "pkg"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("firm/ok"))
        .stdout(predicate::str::contains("2 passed, 1 failed"));
}

#[test]
fn rules_test_inferential_without_judge_is_skipped() {
    let dir = TempDir::new().unwrap();
    let rule = write(
        &dir,
        "no-fluff.yaml",
        concat!(
            "id: no-fluff\n",
            "engine: inferential\n",
            "severity: warning\n",
            "granularity: sentence\n",
            "rubric: Flag contentless fluff.\n",
            "flag_examples: [\"a a\", \"b b\", \"c c\"]\n",
            "pass_examples: [\"x x\", \"y y\", \"z z\"]\n",
        ),
    );
    cmd(&dir)
        .args(["rules", "test", rule.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("skip"))
        .stdout(predicate::str::contains("0 failed, 6 skipped"));
}

#[test]
fn rules_test_ignores_config_enabled_judge() {
    // Regression: a lawlint.config.json that enables the judge (e.g. for
    // editor lint runs) must not make `rules test` download models or call
    // cloud backends — the judge runs only under an explicit --judge flag.
    let dir = TempDir::new().unwrap();
    write(
        &dir,
        "lawlint.config.json",
        r#"{"judge": {"enabled": true, "model": "anthropic:claude-x"}}"#,
    );
    let rule = write(
        &dir,
        "no-fluff.yaml",
        concat!(
            "id: no-fluff\n",
            "engine: inferential\n",
            "severity: warning\n",
            "granularity: sentence\n",
            "rubric: Flag contentless fluff.\n",
            "flag_examples: [\"a a\", \"b b\", \"c c\"]\n",
            "pass_examples: [\"x x\", \"y y\", \"z z\"]\n",
        ),
    );
    cmd(&dir)
        .args(["rules", "test", rule.to_str().unwrap()])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("run with --judge")) // skipped, not judged
        .stdout(predicate::str::contains("0 failed, 6 skipped"))
        // No judge build was even attempted (a build would warn on stderr).
        .stderr(predicate::str::contains("judge unavailable").not());
}

#[test]
fn rules_test_offline_flag_skips_inferential_examples() {
    // Design doc §11: `rules test <dir> --offline` is a documented skip flag.
    let dir = TempDir::new().unwrap();
    let rule = write(
        &dir,
        "no-fluff.yaml",
        concat!(
            "id: no-fluff\n",
            "engine: inferential\n",
            "severity: warning\n",
            "granularity: sentence\n",
            "rubric: Flag contentless fluff.\n",
            "flag_examples: [\"a a\", \"b b\", \"c c\"]\n",
            "pass_examples: [\"x x\", \"y y\", \"z z\"]\n",
        ),
    );
    cmd(&dir)
        .args(["rules", "test", rule.to_str().unwrap(), "--offline"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("0 failed, 6 skipped"));
    // --offline contradicts --judge; clap rejects the combination.
    cmd(&dir)
        .args([
            "rules",
            "test",
            rule.to_str().unwrap(),
            "--offline",
            "--judge",
        ])
        .assert()
        .code(2);
}

#[test]
fn rules_test_missing_path_is_exit_2() {
    let dir = TempDir::new().unwrap();
    cmd(&dir)
        .args(["rules", "test", "no-such-dir"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no-such-dir"));
}

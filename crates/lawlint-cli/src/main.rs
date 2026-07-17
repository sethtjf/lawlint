use clap::{Parser, Subcommand};
use colored::Colorize;
use lawlint_core::{built_in_rules, lint, LintOptions, LintResult, Severity};
use std::collections::HashMap;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(name = "lawlint", about = "Lint AI-generated legal and general text.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(value_name = "FILE", default_value = "-")]
    file: String,
    #[arg(long, value_parser = ["pretty", "json"], default_value = "pretty")]
    format: String,
    #[arg(long, value_name = "ID,ID", value_delimiter = ',')]
    rules: Option<Vec<String>>,
    #[arg(long, value_name = "ID,ID", value_delimiter = ',')]
    disable: Option<Vec<String>>,
    #[arg(long)]
    markdown: bool,
    #[arg(long, default_value = "inf")]
    max_warnings: String,
    #[arg(long)]
    quiet: bool,
}

#[derive(Debug, Subcommand)]
enum Command {
    Rules {
        #[arg(long)]
        json: bool,
    },
}

fn find_config(mut directory: PathBuf) -> LintOptions {
    loop {
        let path = directory.join("lawlint.config.json");
        if let Ok(value) = fs::read_to_string(path) {
            if let Ok(options) = serde_json::from_str(&value) {
                return options;
            }
        }
        if !directory.pop() {
            return LintOptions::default();
        }
    }
}

fn merge_options(mut config: LintOptions, cli: LintOptions) -> LintOptions {
    if cli.enable.is_some() {
        config.enable = cli.enable;
    }
    if cli.disable.is_some() {
        config.disable = cli.disable;
    }
    if cli.markdown.is_some() {
        config.markdown = cli.markdown;
    }
    config.severity = merge_map(config.severity, cli.severity);
    config.thresholds = merge_map(config.thresholds, cli.thresholds);
    config
}

fn merge_map<T>(
    base: Option<HashMap<String, T>>,
    override_map: Option<HashMap<String, T>>,
) -> Option<HashMap<String, T>> {
    match (base, override_map) {
        (None, None) => None,
        (base, Some(overrides)) => {
            let mut merged = base.unwrap_or_default();
            merged.extend(overrides);
            Some(merged)
        }
        (base, None) => base,
    }
}

fn cli_options(cli: &Cli, markdown: Option<bool>) -> LintOptions {
    LintOptions {
        enable: cli.rules.clone(),
        disable: cli.disable.clone(),
        markdown,
        ..Default::default()
    }
}

fn read_input(file: &str) -> Result<(String, bool), String> {
    if file == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| format!("failed to read stdin: {error}"))?;
        return Ok((text, false));
    }
    let path = Path::new(file);
    let text = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    Ok((
        text,
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md")),
    ))
}

fn format_json(result: &LintResult) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(result).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn colors_enabled() -> bool {
    io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn pretty(result: &LintResult, quiet: bool) {
    if quiet {
        return;
    }
    let color = colors_enabled();
    if result.diagnostics.is_empty() {
        let check = "✓ No issues found.";
        println!(
            "{}",
            if color {
                check.green().to_string()
            } else {
                check.into()
            }
        );
    }
    for diagnostic in &result.diagnostics {
        let location = format!("{}:{}", diagnostic.line, diagnostic.column);
        let location = if color {
            match diagnostic.severity {
                Severity::Error => location.red().to_string(),
                Severity::Info => location.cyan().to_string(),
                Severity::Warning => location.yellow().to_string(),
            }
        } else {
            location
        };
        let rule = if color {
            diagnostic.rule_id.bold().to_string()
        } else {
            diagnostic.rule_id.clone()
        };
        println!("{location} {rule} {}", diagnostic.message);
        let excerpt = if color {
            diagnostic.excerpt.dimmed().to_string()
        } else {
            diagnostic.excerpt.clone()
        };
        println!("  {excerpt}");
        if let Some(suggestion) = &diagnostic.suggestion {
            let value = format!("Suggestion: {suggestion}");
            println!(
                "  {}",
                if color {
                    value.dimmed().to_string()
                } else {
                    value
                }
            );
        }
    }
    println!(
        "\nHuman-likeness score: {}/100 ({} words, {} sentences)",
        result.stats.score, result.stats.word_count, result.stats.sentence_count
    );
}

fn lint_file(cli: &Cli) -> Result<LintResult, String> {
    let (text, file_markdown) = read_input(&cli.file)?;
    let config = find_config(std::env::current_dir().map_err(|error| error.to_string())?);
    let markdown = if cli.file == "-" {
        cli.markdown.then_some(true)
    } else {
        Some(cli.markdown || file_markdown)
    };
    let options = merge_options(config, cli_options(cli, markdown));
    Ok(lint(&text, &options))
}

fn run(cli: Cli) -> Result<i32, String> {
    if let Some(Command::Rules { json }) = cli.command {
        if json {
            let rules: Vec<_> = built_in_rules()
                .into_iter()
                .map(|rule| serde_json::json!({ "id": rule.id(), "meta": rule.meta() }))
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&rules).map_err(|error| error.to_string())?
            );
            return Ok(0);
        }
        return Err("the rules subcommand requires --json".into());
    }
    let result = lint_file(&cli)?;
    if cli.format == "json" {
        format_json(&result)?;
    } else {
        pretty(&result, cli.quiet);
    }
    let warnings = result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Warning)
        .count();
    let max_warnings = if cli.max_warnings == "inf" {
        None
    } else {
        cli.max_warnings.parse::<f64>().ok()
    };
    Ok(
        if result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
            || max_warnings.is_some_and(|limit| warnings as f64 > limit)
        {
            1
        } else {
            0
        },
    )
}

fn main() {
    match run(Cli::parse()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("lawlint: {error}");
            std::process::exit(2);
        }
    }
}

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    pub line: usize,
    pub column: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<usize>,
    pub excerpt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub word_count: usize,
    pub sentence_count: usize,
    pub score: i32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LintResult {
    pub diagnostics: Vec<Diagnostic>,
    pub stats: Stats,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuleExample {
    pub bad: String,
    pub good: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuleMeta {
    pub description: String,
    pub docs_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<RuleExample>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LintOptions {
    pub enable: Option<Vec<String>>,
    pub disable: Option<Vec<String>>,
    pub severity: Option<std::collections::HashMap<String, Severity>>,
    pub thresholds: Option<std::collections::HashMap<String, f64>>,
    pub markdown: Option<bool>,
}

pub trait Rule: Send + Sync {
    fn id(&self) -> &str;
    fn meta(&self) -> &RuleMeta;
    fn check(&self, ctx: &RuleContext<'_>) -> Vec<Diagnostic>;
}

pub struct RuleContext<'a> {
    pub text: &'a str,
    pub lines: Vec<&'a str>,
    pub line_starts: Vec<usize>,
    pub options: &'a LintOptions,
}

impl<'a> RuleContext<'a> {
    fn location(&self, offset: usize) -> (usize, usize) {
        let line = self.line_starts.partition_point(|&start| start <= offset);
        let line = line.max(1);
        (
            line,
            self.text[self.line_starts[line - 1]..offset]
                .encode_utf16()
                .count()
                + 1,
        )
    }
    pub fn diagnostic(
        &self,
        start: usize,
        end: usize,
        message: impl Into<String>,
        suggestion: Option<String>,
        severity: Option<Severity>,
    ) -> Diagnostic {
        let (line, column) = self.location(start);
        let (end_line, end_column) = self.location(end);
        Diagnostic {
            rule_id: String::new(),
            severity: severity.unwrap_or(Severity::Warning),
            message: message.into(),
            suggestion,
            line,
            column,
            end_line: Some(end_line),
            end_column: Some(end_column),
            excerpt: self.lines.get(line - 1).unwrap_or(&"").trim().to_string(),
        }
    }
}

fn compiled(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("invalid rule regex {pattern}: {e}"))
}
fn meta(id: &str, description: &str, severity: Severity, rationale: Option<&str>) -> RuleMeta {
    RuleMeta {
        description: description.into(),
        docs_url: format!("https://lawlint.dev/rules/{id}"),
        rationale: rationale.map(str::to_string),
        severity: Some(severity),
        examples: None,
    }
}
struct PhraseRule {
    id: String,
    meta: RuleMeta,
    items: Vec<(Regex, String, Option<String>)>,
    severity: Severity,
}
impl Rule for PhraseRule {
    fn id(&self) -> &str {
        &self.id
    }
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }
    fn check(&self, c: &RuleContext<'_>) -> Vec<Diagnostic> {
        self.items
            .iter()
            .flat_map(|(re, msg, suggestion)| {
                re.find_iter(c.text)
                    .map(|m| {
                        c.diagnostic(
                            m.start(),
                            m.end(),
                            msg,
                            suggestion.clone(),
                            Some(self.severity),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}
struct DensityRule {
    id: String,
    meta: RuleMeta,
    re: Regex,
    threshold: f64,
    message: String,
}
impl Rule for DensityRule {
    fn id(&self) -> &str {
        &self.id
    }
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }
    fn check(&self, c: &RuleContext<'_>) -> Vec<Diagnostic> {
        let matches: Vec<_> = self.re.find_iter(c.text).collect();
        let words = c.text.split_whitespace().count().max(1) as f64;
        let threshold = c
            .options
            .thresholds
            .as_ref()
            .and_then(|x| x.get(&self.id))
            .copied()
            .unwrap_or(self.threshold);
        if matches.is_empty() || (matches.len() as f64 / words) * 1000.0 <= threshold {
            return vec![];
        }
        let m = matches[0];
        vec![c.diagnostic(
            m.start(),
            m.end().max(m.start() + 1),
            &self.message,
            None,
            None,
        )]
    }
}
struct LeadingRule {
    id: String,
    meta: RuleMeta,
    needles: Vec<Regex>,
    message: String,
    suggestion: String,
}
impl Rule for LeadingRule {
    fn id(&self) -> &str {
        &self.id
    }
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }
    fn check(&self, c: &RuleContext<'_>) -> Vec<Diagnostic> {
        let mut out = vec![];
        for needle in &self.needles {
            let re = compiled(&format!(
                r#"(?i)(^|[.!?]["')\]]?\s+|\n\s*)({})"#,
                needle.as_str()
            ));
            for m in re.captures_iter(c.text) {
                let full = m.get(0).unwrap();
                let target = m.get(2).unwrap();
                out.push(c.diagnostic(
                    target.start(),
                    target.end(),
                    &self.message,
                    Some(self.suggestion.clone()),
                    Some(Severity::Error),
                ));
                let _ = full;
            }
        }
        out
    }
}

fn phrase(
    id: &str,
    description: &str,
    severity: Severity,
    items: &[(&str, &str, Option<&str>)],
) -> Box<dyn Rule> {
    Box::new(PhraseRule {
        id: id.into(),
        meta: meta(
            id,
            description,
            severity,
            Some(
                "Avoid patterns that can make otherwise clear prose sound formulaic or overworked.",
            ),
        ),
        severity,
        items: items
            .iter()
            .map(|(p, m, s)| (compiled(p), (*m).into(), s.map(str::to_string)))
            .collect(),
    })
}
fn density(
    id: &str,
    description: &str,
    pattern: &str,
    threshold: f64,
    message: &str,
) -> Box<dyn Rule> {
    Box::new(DensityRule { id: id.into(), meta: meta(id, description, Severity::Warning, Some("Use this signal as a prompt to revise rhythm and density, not as a hard prohibition.")), re: compiled(pattern), threshold, message: message.into() })
}
fn leading(
    id: &str,
    description: &str,
    needles: &[&str],
    message: &str,
    suggestion: &str,
) -> Box<dyn Rule> {
    Box::new(LeadingRule {
        id: id.into(),
        meta: meta(
            id,
            description,
            Severity::Error,
            Some("Start with the substance. Openers that add no information should be cut."),
        ),
        needles: needles.iter().map(|x| compiled(x)).collect(),
        message: message.into(),
        suggestion: suggestion.into(),
    })
}

pub fn built_in_rules() -> Vec<Box<dyn Rule>> {
    let p = |id, d, items| phrase(id, d, Severity::Warning, items);
    vec![
        p("no-ai-cliches","Flags common AI-writing clichés.", &[ (r"(?i)\bdelve\b","Avoid the AI-writing cliché “delve”.",Some("Use a direct verb such as “examine”.")),(r"(?i)\btapestry\b","Avoid the metaphor “tapestry” in analytical prose.",None),(r"(?i)\blandscape of\b","Avoid the vague phrase “landscape of”.",None),(r"(?i)\bin today's fast-paced world\b","Avoid this generic introductory phrase.",None),(r"(?i)\bit is important to note\b","State the important point directly.",None),(r"(?i)\bnavigate the complexities\b","Use a concrete description of the task or issue.",None)]),
        density("no-robotic-transitions","Flags overuse of formulaic transitions",r"(?im)^\s*(Moreover|Furthermore|Additionally|In conclusion),",18.0,"Formulaic sentence transitions are overused."),
        phrase("no-legalese","Flags archaic or unnecessarily formal legalese.",Severity::Warning,&[(r"(?i)\bhereinafter\b","Avoid “hereinafter”.",Some("Name the party or concept directly.")),(r"(?i)\baforementioned\b","Avoid “aforementioned”.",Some("Repeat the noun or use a clear reference.")),(r"(?i)\bpursuant to\b","Consider replacing “pursuant to”.",Some("Use “under” or “by”.")),(r"(?i)\bnotwithstanding the foregoing\b","Avoid “notwithstanding the foregoing”.",Some("State the exception directly.")),(r"(?i)\bherein\b|\bthereto\b","Avoid archaic legalese.",Some("Use a specific noun or pronoun."))]),
        density("no-em-dash-overuse","Flags excessive em dashes",r"—",8.0,"Em dashes are used too frequently."),
        density("no-rule-of-three","Flags dense repeated triplet constructions",r"(?i)\b\w+(?:\s+\w+){0,3},\s+\w+(?:\s+\w+){0,3},\s+and\s+\w+",12.0,"Repeated rule-of-three constructions can sound formulaic."),
        phrase("no-not-only","Flags not-only/but-also constructions",Severity::Warning,&[(r"(?is)\bnot only\b[\s\S]{0,120}\bbut also\b","Avoid the formulaic “not only ... but also” construction.",None)]),
        Box::new(SentenceLengthRule { meta: meta("sentence-length","Flags sentences that are difficult to read.",Severity::Warning,None) }),
        Box::new(RepetitiveRule { meta: meta("no-repetitive-openers","Flags repeated sentence openings.",Severity::Warning,None) }),
        density("no-passive-overuse","Flags likely passive-voice overuse",r"(?i)\b(?:is|are|was|were|be|been|being)\s+\w+(?:ed|en)\b",25.0,"Passive voice appears frequently; prefer active constructions where possible."),
        density("no-hedging","Flags excessive hedging language",r"(?i)\b(?:arguably|it could be said|generally speaking|perhaps|likely)\b",10.0,"Reduce hedging and make the claim more direct."),
        density("no-empty-emphasis","Flags overused empty emphasis words",r"(?i)\b(?:very|really|significantly|crucially)\b",12.0,"Replace emphasis with a specific fact or omit it."),
        phrase("no-doublets","Flags legal doublets and triplets",Severity::Info,&[(r"(?i)\b(?:cease and desist|null and void|any and all)\b","This legal doublet is often unnecessary.",Some("Use one precise term."))]),
        phrase("no-em-dash","Flags every em dash",Severity::Error,&[(r"—","Never use em dashes.",Some("Substitute a comma, period, colon, or parentheses depending on the relationship."))]),
        Box::new(EnDashRule { meta: meta("no-en-dash","Flags en dashes outside numeric ranges.",Severity::Error,Some("En dashes belong only in numeric ranges such as 2020–2024. Elsewhere they read as stray punctuation.")) }),
        phrase("no-semicolons","Flags semicolons.",Severity::Error,&[(";", "Prefer periods over semicolons.",Some("Two short sentences beat one stitched-together one."))]),
        phrase("oxford-comma","Flags lists that omit the Oxford comma.",Severity::Warning,&[(r"(?i)\w+,\s+\w+(?:\s+\w+){0,3}\s+(?:and|or)\s+\w+","Use the Oxford comma before the final item in a list.",Some("Add a comma before the closing “and” or “or”."))]),
        phrase("no-marketing-language","Flags marketing language, hype, and filler.",Severity::Error,&[(r"(?i)\bleverage\b","Avoid the marketing verb “leverage”.",Some("Use “use” or a concrete verb.")),(r"(?i)\bunlock\b","Avoid the hype verb “unlock”.",Some("Describe the actual outcome.")),(r"(?i)\bpowerful\b","Avoid the filler adjective “powerful”.",Some("State the specific capability.")),(r"(?i)\bseamless(?:ly)?\b","Avoid the hype word “seamless”.",Some("Describe what actually happens.")),(r"(?i)\brobust\b","Avoid the filler adjective “robust”.",Some("Name the concrete property.")),(r"(?i)\bcutting[- ]edge\b","Avoid the hype phrase “cutting-edge”.",Some("Say what it is.")),(r"(?i)\bdelve\b","Avoid “delve”.",Some("Use a direct verb such as “examine”.")),(r"(?i)\btapestry\b","Avoid the metaphor “tapestry”.",None),(r"(?i)\bin the realm of\b","Avoid “in the realm of”.",Some("Name the subject directly.")),(r"(?i)\bnavigate the landscape of\b","Avoid “navigate the landscape of”.",Some("Describe the task.")),(r"(?i)\bit['’]s worth noting that\b","State the point directly.",Some("Drop the throat-clearing.")),(r"(?i)\bat the end of the day\b","Avoid the filler phrase “at the end of the day”.",None)]),
        leading("no-sycophantic-openers","Flags sycophantic openers",&["(?:great|good|excellent|fantastic|wonderful) question","what a (?:great|fascinating|wonderful|excellent|interesting) (?:question|problem|point)","that['’]s a (?:great|fascinating|wonderful|excellent) (?:question|point)"],"Skip the sycophantic opener and start with the substance.","Skip the sycophantic opener and start with the substance."),
        leading("no-throat-clearing","Flags throat-clearing openers",&["let me think(?: about this)?","here['’]s my take","here['’]s what i think","i think it['’]s worth","before (?:i|we) (?:begin|start|dive in)"],"Cut the throat-clearing and lead with the point.","Cut the throat-clearing and lead with the point."),
        density("no-parenthetical-asides","Flags frequent parenthetical asides",r"\([^)]*\)",15.0,"Parenthetical asides appear frequently; integrate important clauses into the sentence."),
    ]
}

struct SentenceLengthRule {
    meta: RuleMeta,
}
impl Rule for SentenceLengthRule {
    fn id(&self) -> &str {
        "sentence-length"
    }
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }
    fn check(&self, c: &RuleContext<'_>) -> Vec<Diagnostic> {
        let re = compiled(r"(?s)[^.!?]+[.!?]+|[^.!?]+$");
        re.find_iter(c.text)
            .filter_map(|m| {
                let n = m.as_str().split_whitespace().count();
                let t = c
                    .options
                    .thresholds
                    .as_ref()
                    .and_then(|x| x.get("sentence-length"))
                    .copied()
                    .unwrap_or(45.0);
                (n as f64 > t).then(|| {
                    c.diagnostic(
                        m.start(),
                        m.end(),
                        format!("Sentence is {n} words; consider shortening it."),
                        None,
                        None,
                    )
                })
            })
            .collect()
    }
}
struct RepetitiveRule {
    meta: RuleMeta,
}
impl Rule for RepetitiveRule {
    fn id(&self) -> &str {
        "no-repetitive-openers"
    }
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }
    fn check(&self, c: &RuleContext<'_>) -> Vec<Diagnostic> {
        let re = compiled(r"(?i)(?:^|[.!?]\s+)([A-Za-z']+)");
        let s: Vec<_> = re.captures_iter(c.text).filter_map(|x| x.get(1)).collect();
        (2..s.len())
            .filter_map(|i| {
                let a = s[i - 2].as_str().to_ascii_lowercase();
                (a == s[i - 1].as_str().to_ascii_lowercase()
                    && a == s[i].as_str().to_ascii_lowercase())
                .then(|| {
                    c.diagnostic(
                        s[i - 2].start(),
                        s[i - 2].end(),
                        format!("Three consecutive sentences begin with “{a}”."),
                        None,
                        None,
                    )
                })
            })
            .collect()
    }
}
struct EnDashRule {
    meta: RuleMeta,
}
impl Rule for EnDashRule {
    fn id(&self) -> &str {
        "no-en-dash"
    }
    fn meta(&self) -> &RuleMeta {
        &self.meta
    }
    fn check(&self, c: &RuleContext<'_>) -> Vec<Diagnostic> {
        c.text
            .match_indices('–')
            .filter_map(|(i, _)| {
                let b = c.text[..i].chars().next_back();
                let a = c.text[i + 3..].chars().next();
                (b.is_none_or(|x| !x.is_ascii_digit()) || a.is_none_or(|x| !x.is_ascii_digit()))
                    .then(|| {
                        c.diagnostic(
                            i,
                            i + 3,
                            "Avoid en dashes except in numeric ranges (e.g. 2020–2024).",
                            Some("Use a hyphen, or reword the sentence.".into()),
                            Some(Severity::Error),
                        )
                    })
            })
            .collect()
    }
}

pub fn strip_markdown_code_blocks(text: &str) -> String {
    compiled(r"```[\s\S]*?```")
        .replace_all(text, |m: &regex::Captures| {
            m[0].replace(|c: char| c != '\n', " ")
        })
        .into_owned()
}

pub fn lint(text: &str, options: &LintOptions) -> LintResult {
    let rules = built_in_rules();
    lint_with_rules(text, options, &rules)
}

pub fn lint_with_rules(text: &str, options: &LintOptions, rules: &[Box<dyn Rule>]) -> LintResult {
    let owned = if options.markdown.unwrap_or(false) {
        strip_markdown_code_blocks(text)
    } else {
        text.to_string()
    };
    let text = owned.as_str();
    let mut starts = vec![0];
    for (i, c) in text.char_indices() {
        if c == '\n' {
            starts.push(i + 1)
        }
    }
    let lines = text.split('\n').collect();
    let ctx = RuleContext {
        text,
        lines,
        line_starts: starts,
        options,
    };
    let mut diagnostics = vec![];
    for rule in rules {
        if options
            .disable
            .as_ref()
            .is_some_and(|x| x.iter().any(|id| id == rule.id()))
            || options
                .enable
                .as_ref()
                .is_some_and(|x| !x.iter().any(|id| id == rule.id()))
        {
            continue;
        }
        for mut d in rule.check(&ctx) {
            d.rule_id = rule.id().into();
            if let Some(s) = options.severity.as_ref().and_then(|x| x.get(rule.id())) {
                d.severity = *s
            }
            diagnostics.push(d)
        }
    }
    let word_re = compiled(r"(?u)\b[\w’'-]+\b");
    let word_count = word_re.find_iter(text).count();
    let sentence_count = text
        .split(['.', '!', '?'])
        .filter(|s| !s.trim().is_empty())
        .count();
    let penalty: usize = diagnostics
        .iter()
        .map(|d| match d.severity {
            Severity::Error => 5,
            Severity::Warning => 3,
            Severity::Info => 1,
        })
        .sum();
    let score = (100.0 - (penalty as f64 / (word_count.max(1) as f64)) * 100.0)
        .round()
        .clamp(0.0, 100.0) as i32;
    LintResult {
        diagnostics,
        stats: Stats {
            word_count,
            sentence_count,
            score,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn has(text: &str, id: &str) -> bool {
        lint(text, &LintOptions::default())
            .diagnostics
            .iter()
            .any(|d| d.rule_id == id)
    }
    #[test]
    fn registry() {
        assert_eq!(built_in_rules().len(), 20);
        assert!(built_in_rules().iter().all(|r| r.meta().severity.is_some()));
    }
    #[test]
    fn basics() {
        assert!(has("We should delve into this issue.", "no-ai-cliches"));
        assert!(has("The parties are Alice, Bob and Carol.", "oxford-comma"));
        assert!(!has("The range spans 2020–2024.", "no-en-dash"));
    }
    #[test]
    fn options() {
        let o = LintOptions {
            disable: Some(vec!["no-ai-cliches".into()]),
            ..Default::default()
        };
        assert!(lint("delve", &o)
            .diagnostics
            .iter()
            .all(|d| d.rule_id != "no-ai-cliches"));
    }

    #[test]
    fn accepts_explicit_rule_lists() {
        let rules = built_in_rules();
        let result = lint_with_rules("We delve.", &LintOptions::default(), &rules[..1]);
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].rule_id, "no-ai-cliches");
    }

    #[test]
    fn fixture_parity() {
        let bad = include_str!("../../../packages/lawlint/tests/fixtures/bad.md");
        let bad_result = lint(
            bad,
            &LintOptions {
                markdown: Some(true),
                ..Default::default()
            },
        );
        assert_eq!(
            bad_result.stats,
            Stats {
                word_count: 70,
                sentence_count: 3,
                score: 60
            }
        );
        assert_eq!(
            bad_result
                .diagnostics
                .iter()
                .map(|d| d.rule_id.as_str())
                .collect::<Vec<_>>(),
            vec![
                "no-ai-cliches",
                "no-ai-cliches",
                "no-ai-cliches",
                "no-legalese",
                "no-legalese",
                "no-passive-overuse",
                "no-doublets",
                "no-doublets",
                "oxford-comma",
                "no-marketing-language",
            ]
        );
        let clean = include_str!("../../../packages/lawlint/tests/fixtures/clean.txt");
        let clean_result = lint(clean, &LintOptions::default());
        assert!(clean_result.diagnostics.is_empty());
        assert_eq!(
            clean_result.stats,
            Stats {
                word_count: 20,
                sentence_count: 2,
                score: 100
            }
        );
    }

    #[test]
    fn json_shape_uses_typescript_names() {
        let result = lint("We delve.", &LintOptions::default());
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["diagnostics"][0]["ruleId"], "no-ai-cliches");
        assert_eq!(json["diagnostics"][0]["endLine"], 1);
        assert_eq!(json["diagnostics"][0]["endColumn"], 9);
        assert_eq!(json["stats"]["wordCount"], 2);
        assert_eq!(json["stats"]["sentenceCount"], 1);
    }
}

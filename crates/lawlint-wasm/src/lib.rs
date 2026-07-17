use lawlint_core::{built_in_rules, lint as core_lint, LintOptions};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct RuleInfo<'a> {
    id: &'a str,
    meta: &'a lawlint_core::RuleMeta,
}

#[wasm_bindgen]
pub fn lint(text: &str, options: JsValue) -> Result<JsValue, JsValue> {
    let options = if options.is_null() || options.is_undefined() {
        LintOptions::default()
    } else {
        serde_wasm_bindgen::from_value(options)
            .map_err(|error| JsValue::from_str(&format!("invalid options: {error}")))?
    };
    serde_wasm_bindgen::to_value(&core_lint(text, &options))
        .map_err(|error| JsValue::from_str(&format!("failed to serialize lint result: {error}")))
}

#[wasm_bindgen(js_name = builtInRulesMeta)]
pub fn built_in_rules_meta() -> Result<JsValue, JsValue> {
    let rules = built_in_rules();
    let metadata: Vec<_> = rules
        .iter()
        .map(|rule| RuleInfo {
            id: rule.id(),
            meta: rule.meta(),
        })
        .collect();
    serde_wasm_bindgen::to_value(&metadata)
        .map_err(|error| JsValue::from_str(&format!("failed to serialize rules: {error}")))
}

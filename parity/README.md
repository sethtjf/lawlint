# Rust parity goldens

These files record what the Rust engine does today. The TypeScript port must reproduce them.

- `inputs.jsonl`: every input text, keyed by sha256. Other files point at these through `inputId`.
- `parse.jsonl`: block kinds and ranges, sentence ranges and citation flags. Tokens are not recorded.
- `lint.jsonl`: full lint results. Plain corpus cases leave out `excerpt`.
- `fix.jsonl`: text after `apply_fixes`. Corpus cases store only a hash and a `changed` flag.
- `prompt.jsonl`: remediation prompts. In full for the fixtures and the first 25 corpus samples, as a hash and length for the rest.
- `judge_plan.jsonl`, `judge_ground.jsonl`: judge requests, plus how findings are grounded or dropped.
- `rules_meta.jsonl`, `loader_errors.jsonl`: rule metadata and rule loader errors.
- `docx.jsonl`, `docx/`: extracted text and tracked-change output for each fixture in `fixtures/docx/` plus the crate's `tests/fixtures/sample.docx`.

Regenerate: `cargo run --release -p lawlint-eval --bin parity_capture`
Check: `cargo run --release -p lawlint-eval --bin parity_capture -- --check`

If a PR changes Rust behavior on purpose, regenerate the goldens in that same PR.

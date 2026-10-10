# Rust parity goldens

The JSONL files freeze parse ranges and tokens, lint results, applied fixes, remediation prompts, judge planning and grounding, rule metadata, loader errors, and DOCX extraction/revisions. Each case stores its input text so the snapshot remains usable after Rust and its corpus loader are removed.

Regenerate with `cargo run -p lawlint-eval --bin parity_capture`. Verify with `cargo run -p lawlint-eval --bin parity_capture -- --check`.

Any intended Rust behavior change must regenerate the affected goldens in the same PR.

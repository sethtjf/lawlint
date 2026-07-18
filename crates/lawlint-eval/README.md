# lawlint evaluation corpus

The committed JSONL uses snake_case fields. Human-source appenders are
feature-gated so ordinary workspace builds remain dependency-light:

```text
cargo run -p lawlint-eval --features sourcing --bin source_cap
cargo run -p lawlint-eval --features sourcing --bin source_edgar
cargo run -p lawlint-eval --features sourcing --bin source_govinfo
```

Each appender fetches a capped set of public documents, cleans and segments
100–500 word passages, assigns a unique `pair_id`, and appends rows to
`corpus/corpus.jsonl`. Review the appended rows before regenerating
`corpus/baseline.json` with `report --emit-baseline`.

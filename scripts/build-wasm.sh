#!/usr/bin/env bash
set -euo pipefail

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  export PATH="$HOME/.cargo/bin:$PATH"
fi

if ! command -v wasm-pack >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://rustwasm.github.io/wasm-pack/installer/init.sh | sh
  export PATH="$HOME/.cargo/bin:$PATH"
fi

wasm-pack build crates/lawlint-wasm \
  --target web \
  --out-dir ../../apps/website/src/generated/wasm

//! Emit the release target triple as `LAWLINT_TARGET` so the running binary
//! knows which release asset it corresponds to (docs/engine-design.md §11).
//! Cargo sets `TARGET` for build scripts; it equals the release asset triple,
//! e.g. `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`,
//! `x86_64-pc-windows-msvc`.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=LAWLINT_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}

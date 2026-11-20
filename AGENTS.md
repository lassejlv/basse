# Agent notes

- **Never hard-code the product name in code.** Display names, binary names, Docker label/network/volume prefixes, env var prefixes and config paths all come from `crates/common/src/brand.rs`. The only other places the name may appear are the `[[bin]]` names in `crates/cli/Cargo.toml` and `crates/daemon/Cargo.toml`, and user-facing docs such as the README. Crate, module and type names stay neutral (`common`, `client`, `daemon`, `tui`, `cli`). Check with `grep -rni <name> crates`.
- Crate names are plain nouns without a product or `deploy-` prefix.
- The API contract lives in `crates/common/src/api.rs`. Change it there first, then the daemon routes, then the client.
- The TUI follows the Paper design linked in the README. Colors live in `crates/tui/src/theme.rs`; don't add new ones inline.
- Before pushing: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

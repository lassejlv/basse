# Agent notes

- **Never hard-code the product name in code.** Display names, binary names, Docker label/network/volume prefixes, env var prefixes and config paths all come from `crates/common/src/brand.rs`. The only other places the name may appear are the `[[bin]]` names in `crates/cli/Cargo.toml` and `crates/daemon/Cargo.toml`, and user-facing docs such as the README. Crate, module and type names stay neutral (`common`, `client`, `daemon`, `tui`, `cli`). Check with `grep -rni <name> crates`.
- Crate names are plain nouns without a product or `deploy-` prefix.
- The API contract lives in `crates/common/src/api.rs`. Change it there first, then the daemon routes, then the client.
- The TUI follows the Paper design linked in the README. Colors live in `crates/tui/src/theme.rs`; don't add new ones inline.
- **Design new UI in Paper first.** Before you write TUI code for a new screen, panel, dialog or flow, add artboards for it to the Paper file "Basse TUI" (file id `01M7X3Z4P0WETSB6Q8Q6WXA4TR`, linked in the README). Match the existing artboards: 1280×800, JetBrains Mono 13/20, the file's color tokens, and the same top bar and key bar. Then build to match, and put the Paper and real TUI screenshots side by side in the PR. For small changes to an existing screen, update its artboard so Paper stays the source of truth.
- Before pushing: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

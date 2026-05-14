# W1 Build And Config Baseline

Status: accepted on 2026-05-13.

## Validation Commands

Run from `datrina/`.

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passes.
- `bun run typecheck`: passes.
- `bun run build`: passes. Vite reports a non-failing chunk-size warning for the main bundle.
- `cargo fmt --all --check`: passes.
- `cargo check --workspace --all-targets`: passes. Rust reports a non-failing deprecation warning for `tauri_plugin_shell::Shell::open`; replacing it with `tauri-plugin-opener` is not part of W1.

## Notes

- `bun install` was required because `node_modules` and a JS lockfile were absent. This created `bun.lock`.
- `cargo check` was first blocked by missing crates and required network access to populate the local Cargo cache. This created `Cargo.lock`.
- `src-tauri/icons/icon.png` is a minimal placeholder icon required by Tauri's compile-time context generation. Replace it with production icons during packaging polish.
- Bundle packaging is disabled in `src-tauri/tauri.conf.json` for the baseline. Production packaging targets and icon sets should be restored by a packaging-specific task.

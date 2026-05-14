# W11 Opener Plugin Migration

Status: implemented on 2026-05-14.

## Scope Handled

W11 resolves the post-W10 production-readiness residual for deprecated external
URL opening behavior:

- replaced `tauri_plugin_shell::Shell::open` with
  `tauri_plugin_opener::OpenerExt`,
- registered `tauri_plugin_opener::init()` in the Tauri bootstrap,
- replaced the Rust dependency on `tauri-plugin-shell` with
  `tauri-plugin-opener`,
- removed the unused frontend `@tauri-apps/plugin-shell` dependency,
- kept the frontend command contract unchanged: `systemApi.openUrl` still calls
  `open_url` and expects void success semantics,
- removed the resolved deprecation note from README validation notes and the
  residual backlog.

## Files Changed

- `src-tauri/Cargo.toml`
- `src-tauri/src/main.rs`
- `src-tauri/src/commands/system.rs`
- `package.json`
- `bun.lock`
- `README.md`
- `docs/RESIDUAL_BACKLOG.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W11_OPENER_PLUGIN_MIGRATION.md`
- `src-tauri/gen/schemas/acl-manifests.json`
- `src-tauri/gen/schemas/desktop-schema.json`
- `src-tauri/gen/schemas/macOS-schema.json`

## Validation

Run from `datrina/`:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 37 frontend commands match Rust registrations.
- `bun run typecheck`: passed after `bun install` restored local frontend dependencies.
- `bun run build`: passed; Vite reported only the existing non-failing chunk-size warning.
- `cargo fmt --all --check`: passed.
- `cargo check --workspace --all-targets`: passed after Cargo dependency download was allowed; the previous `tauri_plugin_shell::Shell::open` deprecation warning is gone.

## Acceptance Notes

- The `open_url` command remains a Rust-owned Tauri command.
- The command no longer depends on the deprecated shell plugin open path.
- Tauri generated permission schemas now expose opener permissions instead of shell permissions.
- No new product runtime behavior was added.

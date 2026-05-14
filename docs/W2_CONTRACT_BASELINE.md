# W2 Contract Baseline

Status: accepted

## Boundary

The frontend/Rust boundary is:

`src/lib/api.ts -> Tauri invoke -> registered Rust command -> ApiResult<T>`

Every command wrapped in `src/lib/api.ts` must be registered in
`src-tauri/src/lib.rs`. The static check is:

```sh
bun run check:contract
```

## Response Semantics

- `call<T>()` is for required successful data. A successful `data: null` is a
  contract error.
- `callNullable<T>()` is for commands where `null` is a valid success value.
  `get_config` uses this path.
- `callVoid()` is for commands that return `ApiResult<()>`. `open_url` uses
  this path.
- Missing entity getters for dashboards, chat sessions, and workflows return
  `success: false` with a not-found error. They do not use nullable success.

## Request Casing

- Top-level Tauri function arguments are passed with Tauri's frontend casing,
  such as `dashboardId` for Rust `dashboard_id`.
- Nested request structs are Serde payloads and use the Rust model field names.
  Current nested payloads therefore use snake_case, such as `dashboard_id`,
  `server_id`, and `tool_name`.

## Model Ownership

Rust models under `src-tauri/src/models/*` are the MVP schema source of truth.
TypeScript mirrors are maintained manually in `src/lib/api.ts` and guarded by
the static command/fixture check.

Known residual: generated Rust-to-TypeScript types are not implemented in W2.
If model drift becomes frequent, add a later generated-types workstream instead
of changing this boundary ad hoc.

## Validation Run

Executed on 2026-05-13:

- `bun run check:contract` passed: 33 frontend commands match Rust
  registrations.
- `bun run typecheck` passed.
- `bun run build` passed. Vite reported the existing large chunk warning.
- `cargo fmt --all --check` passed.
- `cargo check --workspace --all-targets` passed. Rust reported the existing
  deprecated `tauri_plugin_shell::Shell::open` warning in `system.rs`.
- Tauri config JSON parse check passed.

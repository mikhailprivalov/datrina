# W15 Chat Runtime Replacement And Message Parts Model

Status: implemented with external-provider validation residual

Date: 2026-05-15

## Scope Handled

- Replaced `ChatPanel`'s parallel `messages` plus metadata plus
  `streamTraces` assembly with a local typed message-parts runtime adapter in
  `src/lib/chat/runtime.ts`.
- Kept the dependency choice local: no new chat UI dependency was added. The
  existing Rust-owned provider/tool boundary means a production package such as
  `assistant-ui` would still need a custom Tauri adapter and would not own
  provider calls, MCP lifecycle, tool execution, persistence, or proposal apply.
  The local adapter now provides the needed message-parts state without moving
  runtime ownership into React.
- Added Rust and TypeScript message parts for assistant text, visible reasoning,
  backend-owned opaque reasoning state references, tool calls, tool results,
  Build Chat proposals, recoverable errors, and cancellation.
- Added canonical `agent_event` payloads on the existing `chat:event` envelope:
  run start/finish/error, text delta, reasoning delta/end, tool call start/end,
  tool result, Build Chat proposal, abort/cancel, and recoverable failure.
- Preserved W12-W14 runtime boundaries: React renders and submits input; Rust
  remains the only owner of provider calls, provider secrets, MCP process
  lifecycle, tool execution, policy decisions, persistence, and proposal apply.
- Preserved Build Chat explicit apply semantics. Proposal parts still render a
  preview and call the existing explicit confirmation/apply path only from the
  visible Apply control.

## Runtime Notes

- The W14 event names remain available for compatibility, but new frontend
  state assembly consumes the canonical `agent_event` shape when present.
- Persisted `ChatMessage.parts` is the canonical UI rendering model. Legacy
  persisted messages without parts are converted by the runtime adapter from
  content, visible reasoning metadata, tool calls/results, and proposal
  metadata.
- Provider-visible reasoning is represented only as `visible_reasoning` parts.
  Provider-opaque reasoning state is represented as a backend-owned state
  reference shape, not as displayable hidden chain-of-thought text.
- Tool arguments/results displayed in events and legacy conversion remain
  masked/truncated before React renders them.
- `local_mock` and Ollama keep honest synthetic single-step event behavior; no
  fake live token streaming was added.

## Validation

Commands run from `datrina/` unless noted:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 41 frontend commands match Rust registrations.
- `bun run typecheck`: passed.
- `bun run build`: passed; Vite emitted the existing non-failing chunk-size warning.
- `cargo fmt --all --check` from `src-tauri/`: passed.
- `cargo check --workspace --all-targets` from `src-tauri/`: passed.

## Acceptance State

- Runtime/dependency decision documented: met with a local typed adapter and no
  new dependency.
- `ChatPanel` no longer manually assembles independent reasoning/tool trace
  state from every event kind: met.
- Rust `ChatMessage`/event model can represent text, visible reasoning,
  provider-opaque reasoning state references, tool calls, tool results, Build
  Chat proposals, cancellation, and failures: met.
- No React code path calls providers, MCP, or tools directly: preserved.
- Tool arguments/results/secrets remain masked before display: preserved.
- Build Chat proposal preview and explicit apply confirmation: preserved.
- Local no-key `local_mock` path: preserved as synthetic single-step code path;
  not treated as live provider streaming evidence.
- Real streaming-provider and tool-calling Build Chat acceptance remains a
  residual in this checkout until user-provided credentials/service availability
  are configured.

# W14 Chat Streaming, Reasoning Trace, And Tool Visibility

Status: implemented with external-provider validation residual

Date: 2026-05-15

## Addendum 2026-05-15: Agent phase observability

The chat event envelope now also carries `ChatEventKind::AgentPhase` /
`AgentEvent::AgentPhase` for live agent run observability:

- `mcp_reconnect` started/completed/failed wraps the MCP discovery step in
  `send_message_stream_inner`, so the UI shows immediately that the chat
  is reconnecting MCP servers instead of staring at a silent
  "Waiting…" string.
- `mcp_list_tools` started/completed/failed fires once per enabled stdio
  MCP server during `reconnect_enabled_mcp_servers`, with per-server tool
  counts on completion.
- `provider_request` started/completed/failed wraps the OpenAI-compatible
  streaming call, with content/reasoning/tool-call counts on completion.
- `provider_first_byte` completed fires from inside the streaming
  callback the first time content or reasoning arrives, with elapsed
  time-to-first-byte.
- `tool_resume` started/completed/failed wraps the bounded one-resume
  provider call after a tool execution iteration.

The streaming SSE reader in `AIEngine::complete_openai_compatible_streaming`
also enforces a 60-second first-byte timeout
(`provider_first_byte_timeout`) so a provider that opens the connection
but never sends a chunk now fails honestly with a recoverable
`RunError` instead of hanging on the per-read 180s timeout indefinitely.

The spawn task wrapper in `send_message_stream` is now panic-safe via
`futures::FutureExt::catch_unwind`. A panic anywhere in
`send_message_stream_inner` converts into a `MessageFailed` event so the
UI's `isLoading` state never silently sticks.

Frontend: `ChatPanel` now renders an `AgentTimeline` component above the
assistant text body. Each phase shows a spinner / check / x icon, a label,
an elapsed counter that ticks every 250 ms while running, and the optional
detail string (in red on failure). The timeline auto-collapses to a small
"Agent steps (N)" disclosure as soon as the provider starts streaming
text. The send button is gated on the Tauri listener having finished
registering, eliminating the listener-race edge case that could drop the
`MessageStarted` event.

The legacy non-streaming `send_message` path keeps its own quiet
`chat_tool_specs_silent` wrapper so it does not emit phase events for a
chat session it cannot stream.

## Scope Handled

- Added a typed Rust `chat:event` envelope and mirrored TypeScript types.
- Added `send_message_stream` and `cancel_chat_response` Tauri commands.
- Implemented OpenAI-compatible streaming chat completions in Rust with SSE
  parsing for content deltas, public provider reasoning fields, streamed tool
  call accumulation, optional token usage, and cancellation checks.
- Kept `local_mock` and Ollama on the same event envelope as synthetic
  single-step providers so React does not represent them as live token streams.
- Kept provider calls, provider secrets, MCP lifecycle, tool execution, policy
  checks, and persistence in Rust.
- Rendered assistant deltas, visible reasoning, tool call progress, tool
  results/errors, parsed build proposals, final message completion, and failed
  stream states in `ChatPanel`.
- Preserved build proposal apply semantics: streamed proposal text can preview
  progress, but dashboard changes still require parsed proposal data and
  explicit Apply.

## Runtime Notes

- The bounded W12/W13 one-resume tool loop is unchanged. W14 does not add
  arbitrary multi-step agent behavior.
- Partial stream noise is not persisted. The persisted assistant message records
  the final assistant content, provider/model metadata, token usage when
  available, visible reasoning summaries when provided by the provider, tool
  calls, tool results, and parsed build proposal metadata.
- `cancel_chat_response` sets an in-flight cancellation flag for the session.
  The Rust streaming loop checks it while reading provider chunks; dropping the
  request stream aborts the provider response path for the current command.
- Visible reasoning is accepted only from explicit provider output fields such as
  `reasoning` or `reasoning_content`. W14 does not request or display hidden
  chain-of-thought.
- Tool arguments and result previews are masked/truncated before event display.
  The UI also masks persisted tool call/result previews before rendering.

## Validation

Commands run from `datrina/`:

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`: passed.
- `bun run check:contract`: passed, 41 frontend commands match Rust registrations.
- `bun run typecheck`: passed.
- `bun run build`: passed; Vite emitted the existing non-failing chunk-size warning.
- `cargo fmt --all`: applied formatting.
- `cargo check --workspace --all-targets`: passed.

## Acceptance State

- Typed Rust and TypeScript chat event envelope: met.
- Rust-owned provider streaming path for OpenAI-compatible providers: met in
  code and compile validation.
- React incremental assistant rendering: met for `chat:event` deltas.
- Tool call visibility while running and after completion: met for requested,
  running, success, and error events, with masked previews.
- Visible provider reasoning rendering: met when the provider supplies public
  reasoning fields.
- Build Chat explicit confirmation boundary: preserved.
- Non-streaming provider honesty: met through synthetic single-step events.
- Failed provider stream recoverability: met through `message_failed` events and
  no persisted fake assistant success message.
- Live real streaming-provider and tool-calling Build Chat acceptance remains a
  residual in this checkout until user-provided credentials/service availability
  are configured. `local_mock` is not live W14 acceptance evidence.

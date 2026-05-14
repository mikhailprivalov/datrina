# W6 AI Provider And Chat Boundary

Status: completed

## Scope Handled

- AI provider calls are Rust-mediated through `src-tauri/src/modules/ai.rs`.
- React still talks only through `src/lib/api.ts`; provider secrets are never
  returned by list/add provider responses.
- `send_message` persists the user message, then either calls an enabled
  provider or returns a deterministic unavailable/provider error. It no longer
  creates hidden placeholder assistant success.
- Streaming is not accepted for W6 MVP baseline. Chat is non-streaming; future
  streaming must use typed Tauri events owned by a later event/workflow
  workstream.

## Provider Behavior

- `openrouter`: OpenAI-compatible `/chat/completions`, requires `api_key`.
- `custom`: OpenAI-compatible `/chat/completions`, allows no key for local
  compatible endpoints.
- `ollama`: local Ollama `/api/chat` and `/api/tags`, no key required.
- `local_mock`: deterministic local smoke provider, no key or network required.

`test_provider` returns a structured `ProviderTestResult` with `ok`,
`invalid_config`, or `unavailable` instead of fake success.

## Residuals

- Encrypted keychain storage remains governed by W3 residual policy; W6 keeps
  secrets Rust-side and masked at the command boundary.
- Tool calling is intentionally not wired until W5 acceptance.
- Build/context chat product behavior beyond a single provider response waits
  for W8.

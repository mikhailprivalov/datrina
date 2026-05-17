# W28 Chat UX Hardening And Regression Pass

Status: planned

Date: 2026-05-17

## Context

The chat runtime now has typed message parts, streaming events, visible tool
activity, proposal previews, cost counters, and Build/Context modes. The
remaining problem is product UX: the drawer can feel stateful in surprising
ways and several important actions are technically present but too hidden or
ambiguous.

Observed issues to address:

- Opening Build Chat from the top bar, templates, or Playground can reuse the
  latest matching session; seeded prompts may land in stale context instead of
  an obvious fresh Build run.
- Merely opening chat or switching mode/dashboard can create empty persisted
  sessions and clutter history.
- User-message copy exists, but it is hover-only and overlaps the edit/resend
  affordance.
- Copy, edit, regenerate, cancel, delete, and new-session actions are weak on
  keyboard/touch paths and rely too much on icon-only hover discovery.
- Session init/load/delete/send failures are mostly console-only, so the panel
  can look idle while input does nothing.
- Switching sessions or starting a new chat during streaming silently cancels
  the active run and does not clearly show cancellation progress or recovery.
- Build retry/edit currently uses message truncation and can reuse derived
  session state such as plans, plan status, cost totals, and title; that must be
  treated as a backend-contract risk, not papered over by frontend UI.

This workstream is a chat UX lifecycle pass. It must preserve W15's typed
message-parts runtime and W27's visual design boundary.

## Goal

- Build Chat entrypoints behave predictably:
  - "Build Chat" from the top bar opens an explicit Build chat surface with a
    clear choice between continuing the current dashboard's latest Build
    session and starting a fresh Build run.
  - Template and Playground seeded prompts start in a fresh Build session by
    default unless the user explicitly chooses to append to an existing session.
  - A fresh empty Build session is created only when the user commits to using
    it, not just by opening the drawer.
- Chat history stays clean:
  - Avoid persisted empty sessions created by drawer open, dashboard browsing,
    or mode toggles.
  - Show empty draft rows or local transient state when a session has not yet
    been sent.
  - Prune or hide zero-message abandoned sessions in the sidebar unless they
    are the active local draft.
- Message actions are discoverable and accessible:
  - Copy is available on user and assistant messages without overlapping edit,
    regenerate, or proposal actions.
  - Long assistant responses, tool traces, validation blocks, and proposal JSON
    have copy affordances scoped to the whole message and relevant sub-blocks.
  - Action controls have keyboard focus states, accessible names, and usable
    touch hit areas.
- Streaming and cancellation are honest:
  - New chat, session switch, close, mode switch, and delete during streaming
    either ask for confirmation or show an explicit cancelling transition.
  - The "reset stuck loading" escape hatch is replaced or demoted so it cannot
    silently conflict with late backend events.
  - Provider first-byte timeout, recoverable mid-stream failure, cancel, and
    panic-safe failure states remain visible after the terminal event.
- Errors and empty states are actionable:
  - Session init/load/delete failures render visible retry controls.
  - No-provider, no-dashboard target, create-new-dashboard, and edit-current
    dashboard states are distinct.
  - Send is disabled with a clear remediation path when no session/provider is
    usable, except for the accepted `local_mock` dev/test path.
- Draft and scroll behavior feels stable:
  - Preserve local drafts per mode/dashboard/session while switching context.
  - Keep scroll anchoring stable during streaming, long tool traces, validation
    retries, proposal previews, and regenerated turns.

## Approach

1. Define chat lifecycle states first.
   - Separate local draft state from persisted `ChatSession`.
   - Create sessions lazily on first send, explicit "New chat", or seeded
     prompt launch.
   - Add an explicit `forceNewSession` / `startFresh` frontend path for template
     and Playground Build launches.
   - Keep Build/Context mode changes from auto-creating persisted sessions.

2. Harden Build Chat entrypoints.
   - Update `App` and `TopBar` so top-bar Build Chat does not silently reuse
     stale context without a visible decision.
   - Make `openBuildChatWithPrompt` create or request a fresh Build session
     before applying `initialPrompt`.
   - Preserve the current dashboard target when editing an existing dashboard;
     clearly label new-dashboard mode when no dashboard is active.

3. Clean up message actions.
   - Replace overlapping absolute hover buttons with a stable message action
     row or compact menu.
   - Keep copy for user messages, assistant text, visible reasoning summaries,
     tool result previews, validation diagnostics, and Build proposal previews.
   - Keep edit/resubmit and regenerate disabled while streaming, with visible
     reasons.
   - Do not expose hidden chain-of-thought or unmasked tool secrets through copy.

4. Make cancellation and session switching explicit.
   - Add a local `cancelling` state distinct from `isLoading`.
   - Require confirmation or a visible status when switching/deleting/starting
     during a stream.
   - Ignore late events from a cancelled old session unless that session becomes
     active again and the backend terminal state is still relevant.

5. Add visible error and recovery states.
   - Render inline errors for session init/load/delete/send failures instead of
     console-only logging.
   - Provide retry buttons for init/load and a "try again" path for failed
     sends that does not accidentally reuse stale Build plan state.
   - Persist failed/cancelled assistant turns only if the backend contract is
     extended in this workstream; otherwise clearly document reload limitations.

6. Treat retry/edit semantics honestly.
   - For Context chat, existing truncate/resend can stay if tests prove it is
     visually correct.
   - For Build chat, either add a backend-safe retry/fork/edit command that
     resets derived plan state intentionally, or limit the UI to "fork into new
     Build chat from this prompt" and record full in-session edit as residual.
   - Do not claim durable Build retry/edit correctness while
     `truncate_chat_messages` only truncates message history.

7. Regression smoke the real UX states.
   - Exercise Context and Build modes with no provider, `local_mock`, a failed
     provider response, a cancelled stream, seeded template/Playground prompts,
     a long assistant answer, tool traces, validation-failed proposal, proposal
     preview/apply, and session switch/delete during a run.

## Files

- `src/App.tsx`
- `src/components/layout/TopBar.tsx`
- `src/components/layout/ChatPanel.tsx`
- `src/lib/chat/runtime.ts`
- Optional small chat components under `src/components/chat/`
- Optional focused frontend tests if the repo has or gains a test harness
- `docs/RECONCILIATION_PLAN.md`
- `docs/W28_CHAT_UX_HARDENING.md`

Backend/API files are allowed only for explicitly accepted lifecycle fixes:

- `src/lib/api.ts`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/models/chat.rs`
- `src-tauri/src/modules/storage.rs`

Crossing into backend/API scope must be called out in the handoff with the
exact reason, such as safe Build retry/fork, durable failed assistant turns, or
durable session pinning.

## Validation

- `bun run typecheck`
- `bun run build`
- `bun run check:contract`
- If backend/API files change:
  - `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
    Rust files when unrelated format drift exists.
  - `cargo check --workspace --all-targets`
- Manual running-app smoke:
  - top-bar Build Chat on an existing dashboard with a previous Build session,
  - template seeded Build Chat starts fresh,
  - Playground "Use as widget" starts fresh with the expected prompt,
  - Context chat continues the expected session,
  - no-provider empty state and remediation,
  - `local_mock` send path,
  - failed provider response and retry/recovery UI,
  - cancellation from the send button,
  - session switch/new chat/delete while streaming,
  - user-message copy and edit do not overlap,
  - assistant copy on long Markdown/code/table response,
  - tool trace and validation-failed proposal remain readable and copy-safe,
  - proposal preview/apply confirmation remains explicit,
  - draft preservation across close/reopen, mode switch, and dashboard switch.
- Screenshot or screen-recording evidence for the hard-to-assert states:
  seeded fresh Build session, visible error/retry state, cancellation state,
  and non-overlapping message actions.

## Out of scope

- Replacing the W15 typed message-parts runtime.
- Reopening provider streaming, MCP lifecycle, tool execution, prompt policy,
  validation rules, or Build proposal apply semantics.
- Direct React-side provider, MCP, or tool calls.
- Hidden chain-of-thought display or copying provider-opaque reasoning state.
- W27 cyberpunk visual retheme, global design-token rewrite, app shell redesign,
  or broad widget/dashboard restyling.
- Durable chat pinning, durable draft storage, or history export unless the
  backend/API scope is explicitly accepted inside this workstream.
- Multi-agent orchestration, autonomous actions, alert-triggered sessions, or
  changes to W21/W26 runtime behavior.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W14_CHAT_STREAMING_TRACE_UI.md`
- `docs/W15_CHAT_RUNTIME_REPLACEMENT.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W18_PLAN_EXECUTE_REFLECT.md`
- `docs/W20_DATA_PLAYGROUND_TEMPLATES.md`
- `docs/W22_TOKEN_COST_TRACKING.md`
- `docs/W27_CYBERPUNK_REDESIGN.md`

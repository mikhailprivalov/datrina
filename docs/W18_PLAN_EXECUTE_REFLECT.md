# W18 Plan / Execute / Reflect Orchestration

Status: planned

Date: 2026-05-16

## Context

The agent today runs as one monolithic system prompt that mixes planning,
exploration, pipeline construction, validation, and proposal emission.
`AgentTimeline` (W14) surfaces low-level pipeline phases (MCP reconnect,
provider request, first byte) but **not the agent's own intentions**. Users
cannot see "what is the agent trying to do right now" until it's already
done.

Additionally, after Apply, no one checks whether the widget actually
rendered useful data. A widget showing `0` or `null` after a real refresh
goes unnoticed until the user spots it manually.

## Goal

Two explicit phases bookend every Build run:

1. **Plan**: the agent emits a structured plan as its first tool call,
   visible as a checklist artifact in the UI. The user can read it before
   any tool fires, redirect via chat, or let it run.
2. **Reflect**: after `apply_build_proposal` and the first scheduled
   refresh of new widgets, the system feeds the agent a render snapshot
   and asks it to critique its own output. Fix-up proposals appear
   automatically as suggestions, not auto-applied.

## Approach

### Plan phase

#### Tool spec

New tool `submit_plan(plan)` registered in `chat_tool_specs`:

```jsonc
{
  "name": "submit_plan",
  "description": "Submit your execution plan before doing any other work.",
  "parameters": {
    "steps": [
      {
        "id": "string",            // stable id, e.g. "explore_mcp"
        "title": "string",         // user-facing line, e.g. "List enabled MCP tools"
        "kind": "explore" | "fetch" | "design" | "test" | "propose" | "other",
        "depends_on": ["string"],  // step ids
        "rationale": "string"      // 1 sentence why
      }
    ],
    "summary": "string"            // 1-2 sentence elevator pitch of the whole plan
  }
}
```

Enforcement (`commands/chat.rs`): if the first tool call in a Build chat
session is not `submit_plan`, inject a synthetic tool result
`"plan_required: call submit_plan first to outline your steps"` and force
a provider turn. This makes the plan effectively mandatory without
hard-blocking edge cases (already-planned sessions, continuations).

#### Persistence

New columns on `chat_sessions`: `current_plan_json`, `plan_step_status_json`.
Status JSON is a map `step_id -> ("pending" | "running" | "done" | "failed")`.

Status transitions are driven by:

- `step_id` echoed in each subsequent tool call via a new optional
  `_plan_step: string` argument (system-prompt instruction tells the agent
  to set it).
- On `submit_plan` return → all steps `pending`.
- When a tool call carries `_plan_step` → set that step to `running`, mark
  the previous step `done` if still `running`.
- On `MessageCompleted` → mark final step `done`.
- On `MessageFailed` → mark `running` step `failed`.

#### Event

Extend `ChatEventKind` with `PlanUpdated`. Payload includes the full plan
JSON + current status map. Emitted whenever status changes.

#### UI (`src/components/layout/ChatPanel.tsx`)

A new component `PlanArtifact`:

- Rendered above the message body for the assistant turn that owns the
  plan, persisted with the message.
- Vertical checklist: pending = empty circle, running = spinner, done =
  green check, failed = red x.
- Each step shows title + rationale (collapsed by default, expand to read).
- Summary banner at top.
- "Stop" button cancels the chat (reuses `cancelChatResponse`).

The `AgentTimeline` stays as it is — `PlanArtifact` is the higher-level
intent; the timeline is the lower-level phase trace.

### Reflect phase

#### Trigger

Add a hook after `apply_build_proposal_inner` finalises: for each newly
created or replaced widget, register a one-shot `post_apply_reflection`
job. The job fires after the **first successful `refresh_widget`** for
those widgets (latency ~1–60 s depending on cron / manual refresh).

#### Snapshot

The job collects per-widget:

- Last rendered widget value (or `data` payload, truncated to 4 KB).
- Last pipeline trace summary (from W23 if available; otherwise just
  count of pipeline steps).
- Whether the value is empty/null/zero (heuristic: same type as configured
  but trivially empty for that type).
- Last refresh error string if any.

#### Reflection turn

The job opens (or resumes) a chat session — same dashboard, same chat as
the original proposal — and posts a **synthetic user-role message**:

```
[reflection] The following widgets were applied and have just refreshed:
- widget abc123 (stat "Active release"): value = null, pipeline = 3 steps, no error
- widget def456 (table): rows = 0, no error

Critique your own proposal. For each widget that looks broken, propose a
fix-up BuildProposal (delta only) or explicitly mark it as expected.
Do not call dry_run unless the data path itself looks suspect.
```

The agent's response uses the standard delta proposal format. It is not
auto-applied: the UI shows a "Reflection suggestion" badge on the new
assistant message, with one-click Apply.

#### Skip rules

- Reflection is suppressed if the widget hasn't refreshed within 5 minutes
  (workflow stuck — surfaced through W21 alerts instead).
- Reflection is suppressed if the user already manually edited the widget
  in between (avoid stomping on user intent).
- Reflection budget: one round per Apply, configurable per-dashboard.

## Files to touch

- `src-tauri/src/models/chat.rs` — `PlanArtifact`, `PlanStepStatus`,
  `ChatEventKind::PlanUpdated`, `AgentEvent::PlanUpdated`.
- `src-tauri/src/commands/chat.rs` — `submit_plan` tool spec; plan
  enforcement on first tool call; `_plan_step` tracking on subsequent
  calls; emit `PlanUpdated`.
- `src-tauri/src/commands/dashboard.rs` — `apply_build_proposal_inner`
  registers reflection job per new widget.
- `src-tauri/src/modules/scheduler.rs` — `register_post_apply_reflection`
  hook fired by `refresh_widget` on first successful run.
- `src-tauri/src/commands/chat.rs` — `enqueue_reflection_turn(session_id,
  snapshot)` posts the synthetic user message and triggers a streaming
  agent turn.
- `src-tauri/src/modules/storage.rs` — migration for plan columns.
- `src/lib/api.ts` — mirror types; `PlanArtifact`, `PlanStepStatus`,
  reflection metadata on `ChatMessage`.
- `src/components/layout/ChatPanel.tsx` — `PlanArtifact` rendering;
  "Reflection suggestion" badge style.
- `src/lib/chat/runtime.ts` — handle `PlanUpdated` event; merge plan into
  message state.

## Validation

- Unit: `submit_plan` enforcement — first tool call ≠ submit_plan → loop
  produces a `plan_required` synthetic result.
- Unit: status transitions — given a synthetic stream of tool calls with
  `_plan_step`, the status map ends up consistent.
- Manual: apply a proposal that yields a stat with `value: null` (force a
  bad source). After first refresh, observe the reflection message appear
  in the same session with a delta proposal.
- Manual: cancel a session mid-plan; confirm status remains as-of-last-event.

## Out of scope

- Multi-agent / agent-of-agents. One model, two prompt phases.
- Auto-applying reflection suggestions (always user-gated).
- Plan editing by the user (read-only artifact in v1).

## Related

- W14 — same chat event envelope.
- W16 — validation gate runs **before** the proposal is shown; reflection
  runs **after** Apply. They are complementary.
- W21 — alerts on stuck workflow are the safety net under reflection.

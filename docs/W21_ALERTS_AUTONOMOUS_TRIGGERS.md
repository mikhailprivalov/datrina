# W21 Alerts And Autonomous Triggers

Status: shipped

Date: 2026-05-16

## Context

Dashboards are static surfaces. The user has to look at them to know
something changed. There is no notification path and no way for the
agent to act on its own when data crosses a threshold. The scheduler
already triggers refreshes; nothing reads the result and decides whether
to alert.

This converts Datrina from a dashboard builder into a real monitoring
tool.

## Goal

- Per-widget **alert** definitions (condition + severity + message).
  Evaluated by the scheduler after every refresh. Firing alerts
  produce: an OS notification, a badge on the widget, a badge on the
  Sidebar, and an entry in an alert event log.
- Per-widget **autonomous triggers** (`agent_trigger`): like an alert,
  but instead of just notifying, it opens a background chat session and
  hands the agent a templated prompt ("CPU above 90% on host X. Suggest
  next steps."). Suggestion appears as a notification with "View" deep
  link to the chat.

## Approach

### Alert definition

Extend `WidgetConfig` (`models/widget.rs`) with an optional `alerts`
field shared across widget kinds:

```rust
pub struct WidgetAlert {
    pub id: String,
    pub name: String,
    pub condition: AlertCondition,
    pub severity: AlertSeverity,       // info | warning | critical
    pub message_template: String,      // supports {value}, {path}, {threshold}
    pub cooldown_seconds: u32,         // default 600
    pub enabled: bool,
}

pub enum AlertCondition {
    Threshold {
        path: String,                  // JMESPath-like on widget data
        op: ThresholdOp,               // gt | lt | gte | lte | eq | neq
        value: serde_json::Value,
    },
    PathPresent { path: String, expected: PresenceExpectation }, // present | absent | empty | non_empty
    StatusEquals { path: String, status: String },
    Custom { jmespath_expr: String },  // boolean expression
}

pub enum AlertSeverity { Info, Warning, Critical }
```

Schema migration is one column on `widgets`: `alerts_json TEXT NOT NULL
DEFAULT '[]'`.

### Evaluation

After every successful `refresh_widget`, the scheduler hands the
rendered data (post-pipeline) to a new `AlertEngine::evaluate(widget,
data)`. For each alert:

1. Resolve the path on `data` (reuse `workflow_engine::resolve_path`).
2. Apply the condition.
3. If firing and not within cooldown (compare against the latest
   `alert_events` row for this alert_id), record an event.

### Storage

New table:

```sql
CREATE TABLE alert_events (
  id TEXT PRIMARY KEY,
  widget_id TEXT NOT NULL,
  alert_id TEXT NOT NULL,
  fired_at INTEGER NOT NULL,
  severity TEXT NOT NULL,
  message TEXT NOT NULL,
  context_json TEXT NOT NULL,        -- { value, path, threshold }
  acknowledged_at INTEGER
);
CREATE INDEX alert_events_widget_idx ON alert_events (widget_id, fired_at DESC);
CREATE INDEX alert_events_unack_idx ON alert_events (acknowledged_at) WHERE acknowledged_at IS NULL;
```

### Notifications

Use `tauri-plugin-notification`. Each firing alert produces:

- OS notification (title = widget title, body = rendered message,
  severity color via icon).
- In-app: a red dot badge on the widget header, a count badge on the
  Sidebar item "Alerts (3)".
- Acknowledging an alert (click → mark `acknowledged_at`) clears the
  badge for that one event.

Permission added to `src-tauri/capabilities/default.json`:
`notification:default`.

### Alerts surface

New Sidebar route `#/alerts` showing the unacknowledged alert events
feed. Grouped by widget, sorted by severity then time. Click → jumps to
the dashboard with the offending widget highlighted.

### Autonomous triggers

Triggers reuse the alert plumbing with one new field per widget alert:

```rust
pub struct WidgetAlert {
    /* ...as above... */
    pub agent_action: Option<AgentAction>,
}

pub struct AgentAction {
    pub mode: ChatMode,               // build | context
    pub prompt_template: String,
    pub max_runs_per_day: u32,        // safety budget
    pub allow_apply: bool,            // if false, agent can only suggest, not Apply
}
```

When an alert fires AND `agent_action.is_some()`:

1. Check daily budget (rows in `alert_events` joined with chat sessions
   spawned from this alert).
2. Render the prompt template with alert context.
3. Create a new chat session (`autonomous = true` flag on
   `chat_sessions`).
4. Stream the agent run in the background (same `send_message_stream`
   path; emitted events still flow to the front end so the user can
   watch live if they want).
5. On `MessageCompleted`, fire a follow-up OS notification: "Agent
   suggests X. View".

`allow_apply` defaults to `false`: autonomous proposals never auto-apply
unless the user explicitly enabled it per trigger. This is the safety
boundary.

### UI for alert editing

On each widget kebab → "Alerts" → modal:

- List of alert definitions.
- Form for adding/editing: name, condition picker, severity, message
  template, cooldown, enabled toggle, optional agent action panel.
- Test button: evaluates current condition against the widget's last
  rendered data and shows preview.

## Files to touch

- `src-tauri/src/models/widget.rs` — `WidgetAlert`, `AlertCondition`,
  `AlertSeverity`, `AgentAction`.
- `src-tauri/src/modules/alert_engine.rs` (new).
- `src-tauri/src/modules/scheduler.rs` — invoke `AlertEngine` after
  refresh; spawn autonomous chat sessions for triggers.
- `src-tauri/src/modules/storage.rs` — `alerts_json` column;
  `alert_events` table.
- `src-tauri/src/commands/alert.rs` (new) — `list_alert_events`,
  `acknowledge_alert`, `set_widget_alerts`.
- `src-tauri/src/lib.rs` — register commands; manage `AlertEngine` in
  `AppState`; add notification plugin.
- `src-tauri/capabilities/default.json` — `notification:default`.
- `src-tauri/Cargo.toml` — `tauri-plugin-notification`.
- `src/lib/api.ts` — types + `alertApi`.
- `src/components/alerts/AlertsView.tsx` (new).
- `src/components/alerts/AlertEditorModal.tsx` (new).
- `src/components/layout/Sidebar.tsx` — "Alerts" item with badge count.
- `src/components/layout/DashboardGrid.tsx` — alert dot on widget header;
  kebab → Alerts action.

## Validation

- `bun run check:contract`, `cargo check --workspace --all-targets`.
- Manual: configure an alert on a stat widget (value > 50); manually
  refresh with a data path that returns 100; observe OS notification +
  badges.
- Manual: configure an autonomous trigger with prompt "value spiked,
  suggest fix"; trigger it; observe a background chat session in the
  list with a streaming run, and a follow-up notification.
- Manual: daily budget honoured — fire same alert >`max_runs_per_day`
  times; confirm subsequent agent runs are skipped (alert event still
  recorded, no chat session created).
- Manual: acknowledge an alert; confirm Sidebar badge decrements.

## Out of scope

- Email / Slack / webhook delivery. OS notifications only in v1.
- Auto-apply of autonomous-agent proposals (always user-gated for now).
- Alert grouping / deduplication beyond simple cooldown.
- Anomaly detection (only explicit conditions in v1).

## Related

- W18 reflection — different mechanism, similar shape (post-event agent
  turn). They share the `enqueue_reflection_turn` path internally.
- W19 — autonomous agent runs that apply produce normal dashboard
  versions, so undo still works.
- W22 — autonomous runs share the same token budget as user-initiated
  ones; cost tracking applies.

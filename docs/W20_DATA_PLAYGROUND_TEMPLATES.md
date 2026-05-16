# W20 Data Playground And Onboarding Templates

Status: shipped

Date: 2026-05-16

## What landed

- Standalone `#/playground` route with a three-pane layout (Sources |
  Arguments | Result). Reachable from the Sidebar "Explore (Playground)"
  item.
- MCP sources come from existing `list_tools` (the `input_schema` field
  was already being passed through; no backend change needed there).
- Custom HTTP source backed by a new `execute_http_request` Tauri command
  that wraps `tool_engine.http_request` (reuses the same policy gate the
  chat agent uses).
- Schema-driven argument form for MCP tools (string/number/boolean/enum
  + JSON textarea fallback for nested objects and arrays).
- Result pane tabs: JSON, Table (auto-detects array-of-objects roots up
  to depth 3 with a path picker), Chart (line/bar over the first numeric
  column), Schema sketch.
- "Use as widget" composes a Build Chat prompt with source, args, a 4 KB
  sample, and an optional user note, then opens the chat pre-filled.
- Saved presets persisted in a new `playground_presets` SQLite table via
  `list/save/delete_playground_preset` commands.
- Template Gallery with 8 templates replaces the empty state. Same
  gallery available as a modal from the Sidebar ("From template…").
  Each card flags missing required MCP servers and links into MCP
  settings.

## Out of scope (carried forward)

- Visual chart designer in Playground (basic chart preview only; full
  configuration still happens in the widget editor).
- User-created templates / sharing — registry is static code in v1.
- Cross-tool composition in Playground; chains continue to go through
  Build Chat.

## Context

Two related usability cliffs:

1. **No way to explore data outside the agent flow.** Users who want to see
   what an MCP tool or HTTP endpoint returns have to either ask the agent
   ("describe what get_releases returns") or build a widget through Build
   Chat. `dry_run_widget` requires an already-shaped widget proposal. The
   feedback loop for "what does this tool actually look like" is long and
   indirect.
2. **Empty-state has only `blank | local_mvp`** (`api.ts:757-758`). A new
   user opens Datrina, sees two unfamiliar buttons, and has no path to
   discover what the app is good for.

## Goal

- A standalone **Data Playground** route that lets users pick a server +
  tool (MCP or HTTP), fill in arguments, run, and inspect the response as
  JSON / table / chart preview. A single "Use as widget" button hands the
  exploration off to the Build Chat with the source already wired.
- A **Template Gallery** on dashboard empty state with 6–8 curated
  presets, each seeding a prefilled Build Chat prompt. Removes the cold
  start.

## Approach

### Data Playground

#### Route & shell

Add a new top-level route `#/playground` reachable from the Sidebar
(a "Explore" item below "Dashboards", above "Settings"). Three-pane
layout:

```
┌──────────────┬───────────────────────┬─────────────────────────┐
│ Sources      │ Arguments             │ Result                  │
│ ─ MCP ▾      │ (form generated from  │ Tabs: JSON | Table |    │
│   ─ server-a │  selected tool's      │ Chart | Schema          │
│     ─ list_… │  JSON Schema)         │                         │
│   ─ server-b │                       │ [Use as widget] [Save]  │
│ ─ HTTP       │ [Run] [Reset]         │                         │
│   ─ Custom   │                       │                         │
└──────────────┴───────────────────────┴─────────────────────────┘
```

#### Source listing

`mcpApi.listToolsWithSchema()` — extend the existing `listTools` to also
return `inputSchema` (already part of MCP `tools/list` response, just not
currently surfaced to the front end). For HTTP, a single "Custom HTTP"
entry that uses the existing `http_request` built-in tool.

#### Argument form

Render a form from `inputSchema` (JSON Schema). Use a lightweight schema
renderer; primitive types only in v1 (string, number, boolean, enum,
nested objects via JSON textarea fallback).

Pre-populate from saved presets (see "Save preset" below).

#### Run

Hits the existing tool execution path:

- MCP: `mcpApi.callTool(server_id, tool_name, args)` — already exists.
- HTTP: `toolApi.executeCurl({ method, url, headers, body })` — already
  exists.

Result is shown in the right pane:

- **JSON**: pretty-printed, collapsible, with copy-path-to-clipboard on
  hover (writes JMESPath-like path).
- **Table**: auto-detects array of objects at root or at one of the top
  paths (max depth 3); offers a dropdown to switch the table root path.
- **Chart**: if table has a numeric column, offer a quick line/bar chart;
  configurable axes.
- **Schema**: inferred schema sketch (same shape derivation used by W17's
  `mcp_tool_observed_shape`).

#### "Use as widget"

Opens Build Chat (current dashboard or new dashboard) with a prefilled
prompt:

```
Build a {widget_kind_suggestion} widget for this data path.

Source: MCP server "<server_id>" tool "<tool_name>", args:
{
  "<arg_a>": "...",
  "<arg_b>": "..."
}

Data sketch (one sample):
{ ... pruned to 4 KB ... }

The widget should display: {user note from a textarea}.
```

Widget kind suggestion is chosen heuristically from the JSON shape
(single object → stat / text; array of objects with numeric column →
table or chart; small object with one number → gauge).

This bridges Playground → Build Chat without the agent re-doing
exploration. Combined with W17, the agent will also already know the
tool shape from prior memory.

#### Save preset

A "Save" button next to "Run" stores `(server_id, tool_name, args,
display_name)` to a new table `playground_presets`. Listed in the left
pane under each server. Lets users keep their common queries.

### Template Gallery

#### Registry

New file `src/lib/templates/index.ts` exports a static registry of 6–8
templates, each:

```ts
interface DashboardTemplate {
  id: string;                      // 'github_stats'
  title: string;                   // "GitHub repo stats"
  description: string;             // 1-2 sentence pitch
  icon: string;                    // lucide icon name
  required_mcp: string[];          // server fingerprints, e.g. ['github-mcp']
  prompt: string;                  // Build Chat prefilled prompt
  example_widgets: string[];       // for the card preview
}
```

Initial set (subject to revision after user feedback):

1. `github_repo_stats` — stars/PRs/issues for a repo over time.
2. `crypto_top10` — CoinGecko top-10 by market cap with sparkline.
3. `system_monitor_local` — CPU / mem / disk via a local MCP system tool.
4. `release_status_mcp` — generic project release dashboard driven by a
   user-configured MCP server (release/inventory feed). Reference
   template for "single stat + table over MCP".
5. `http_uptime` — paste a list of URLs, dashboard pings + shows status.
6. `linear_inbox` — recent Linear issues for a workspace.
7. `from_prompt` — open Build Chat with no preset prompt; just a blank
   chat for free-form construction.
8. `from_playground` — opens Playground first.

#### UI

Replace the current dashboard empty state with a grid of cards
(`src/components/onboarding/TemplateGallery.tsx`). Each card:

- Icon + title + description + "required MCP" badges (warning if
  required server isn't configured, with a one-click "Add server" deep
  link to MCP settings).
- Click → either:
  - Creates an empty dashboard + opens Build Chat with `prompt`
    prefilled; or
  - For `from_playground` → routes to `#/playground`.

#### Dashboard menu integration

A small "+ New from template" button in the Sidebar (next to existing
"+ New") opens the same gallery as a modal, so existing users can
create new dashboards from templates without going through the empty
state.

## Files to touch

- `src-tauri/src/commands/mcp.rs` — extend `list_tools` response with
  `input_schema`. (Already retrieved during `tools/list` over JSON-RPC;
  currently stripped — pass it through.)
- `src-tauri/src/modules/storage.rs` — migration for `playground_presets`.
- `src-tauri/src/commands/playground.rs` (new) — `save_preset`,
  `list_presets`, `delete_preset`.
- `src-tauri/src/lib.rs` — register playground commands.
- `src/lib/api.ts` — mirror; add `playgroundApi`.
- `src/components/playground/Playground.tsx` (new) — main shell.
- `src/components/playground/ArgumentsForm.tsx` (new).
- `src/components/playground/ResultPane.tsx` (new) — JSON/Table/Chart/Schema tabs.
- `src/components/onboarding/TemplateGallery.tsx` (new).
- `src/lib/templates/index.ts` (new) — static template registry.
- `src/App.tsx` — register `#/playground` route.
- `src/components/layout/Sidebar.tsx` — Explore item; New-from-template
  modal trigger.
- `src/components/layout/DashboardGrid.tsx` — empty state replacement.

## Validation

- `bun run check:contract` (new playground commands).
- `bun run typecheck`, `bun run build`.
- Manual: open Playground, pick any configured MCP server + tool, fill
  args, run; confirm JSON / Table / Schema tabs render.
- Manual: click "Use as widget"; confirm Build Chat opens with a
  meaningful prefilled prompt and that the agent's first MCP call is
  the same tool.
- Manual: from a fresh app start (delete dashboards), see Template
  Gallery; click any template; build runs.

## Out of scope

- Visual chart designer in Playground (basic chart preview only; full
  configuration happens in widget editor).
- User-created templates / sharing (registry is static code in v1).
- Cross-tool composition (the playground runs one tool at a time; chains
  go through Build Chat).

## Related

- W17 — Playground saves observed tool shapes into `mcp_tool_observed_shape`
  the same way Build Chat does; benefits accrue both ways.
- W18 — "Use as widget" from Playground produces a Build Chat session
  with a pre-formed plan, so the plan phase is one step shorter.

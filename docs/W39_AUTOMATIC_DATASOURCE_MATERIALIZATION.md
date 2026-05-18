# W39 Automatic Datasource Materialization

Status: shipped

Date: 2026-05-17

## Outcome

- Inline single-widget Build datasource plans (builtin_tool / mcp_tool /
  provider_prompt) now auto-materialize into saved `DatasourceDefinition`
  rows on confirmed apply. Workbench shows them immediately without a
  rescan.
- Reuse + dedupe is signature-based: a new
  `modules::datasource_signature::DatasourceSignature` normalises
  kind/server_id/tool_name/arguments/prompt/pipeline (recursive JSON key
  sort) so equivalent shapes collapse onto one definition. Existing
  shared-datasource reuse goes through the same signature so reordered
  LLM JSON no longer creates duplicates.
- `http_request` is treated as a first-class datasource source.
  `validate_http_request_arguments` rejects bad methods, non-public URLs,
  credential headers, and `[object Object]` body strings at three points:
  the apply path, the W16 proposal validation gate (new
  `ValidationIssue::UnsafeHttpDatasource`), and the materialization
  preview.
- New read-only command `preview_proposal_materialization` returns the
  per-source `create / reuse / passthrough / reject` plan; the chat
  proposal preview surfaces it before Apply so the user sees what will
  land in the catalog.
- Compose, plans with `output_path`, and plans with `inputs` are
  intentionally left on the per-widget workflow path (`passthrough` in
  the preview) — they would need additional tail-pipeline plumbing to
  share a saved definition cleanly. Recorded as a follow-up.

## Residuals (deferred)

- Playground "Use as widget" still seeds Build Chat with a prompt
  string. Auto-materialization picks the resulting proposal up, but the
  doc's stated structured-metadata handoff (no pasted prose) is still
  ahead of the current `onUseAsWidget({ prompt, sourceLabel })` shape.
- Compose / output_path / inputs widgets still create per-widget
  workflows. Collapsing them into saved definitions requires moving
  output_path and per-input shaping into the consumer `tail_pipeline`.

## Context

Datrina now has saved datasources, explicit datasource identity, Build Chat
reuse, shared source fan-out, and HTTP/MCP/provider-backed workflow execution.
That is still not enough for the intended product UX: the user should not have
to open Workbench and manually create a datasource before asking Build Chat to
build useful widgets.

The current acceptable runtime shape is explicit and persisted, but the creation
path is still too manual. A Build proposal can contain executable
`datasource_plan` / `shared_datasources` entries, and existing saved
datasources can be reused when signatures match. The missing fix is automatic
materialization: when Build Chat or Playground introduces a new reusable source,
the apply/save path should create or reuse a saved `DatasourceDefinition`
without requiring a separate manual setup step.

This task is about removing manual datasource setup from normal dashboard
creation. It must not weaken explicit apply confirmation, validation, secrets
handling, provenance, or no-fake-success behavior.

## Goal

- Build Chat can create a dashboard from HTTP/MCP/provider sources without the
  user manually creating datasources first.
- Any new reusable Build source is materialized into a saved
  `DatasourceDefinition` during confirmed apply, unless validation rejects it.
- Existing matching saved datasources are reused by default instead of creating
  duplicates.
- HTTP request sources are first-class datasource candidates via
  `kind: "builtin_tool"`, `tool_name: "http_request"`, and structured
  `{method, url, headers?, body?}` arguments.
- Multiple widgets that read the same source in one proposal share one saved
  datasource and apply per-widget tail pipelines on top of it.
- Playground successful HTTP/MCP runs can feed Build Chat or "Use as widget"
  without forcing the user through a separate Workbench setup flow.
- Proposal preview clearly shows whether each source will be created, reused,
  updated, or rejected before explicit apply.
- Workbench remains the inspection/edit/debug surface, not a required setup
  step for the happy path.

## Approach

1. Define a canonical datasource signature.
   - Introduce one reusable signature helper for saved definitions, Build
     `shared_datasources`, inline widget `datasource_plan`, and Playground
     source runs.
   - Include `kind`, `server_id`, `tool_name`, canonicalized `arguments`,
     `prompt`, and base `pipeline`.
   - Ignore display-only fields such as name/description and handle cron as a
     policy decision rather than a source identity field.
   - Canonicalize JSON object key order so equivalent HTTP/MCP arguments dedupe
     reliably.

2. Materialize Build proposal sources on confirmed apply.
   - Before creating widget workflows, resolve every proposal source to one of:
     `reuse_existing`, `create_datasource`, `update_existing`, or `reject`.
   - For `create_datasource`, persist a `DatasourceDefinition`, create its
     backing single-output workflow, and bind all consumer widgets through
     `datasource_definition_id`.
   - For `reuse_existing`, bind widgets to the existing definition and copy
     consumer-specific pipelines into `DatasourceConfig.tail_pipeline`.
   - Do not create hidden duplicate fan-out workflows when a saved datasource can
     represent the source.
   - Preserve the existing shared fan-out path only as a compatibility fallback
     for proposal shapes that cannot yet be materialized safely.

3. Promote inline single-widget datasource plans.
   - If a proposal has one widget with a direct `builtin_tool`, `mcp_tool`, or
     `provider_prompt` `datasource_plan`, materialize that source as a saved
     datasource during apply instead of leaving it as an anonymous widget
     workflow.
   - If several inline plans share the same canonical signature, collapse them
     to one saved definition and separate tail pipelines.
   - Keep widget `output_key` and runtime refresh behavior compatible with the
     existing workflow engine.

4. Make HTTP datasource creation explicit and safe.
   - Treat `http_request` as a built-in datasource source with structured
     arguments, validation, timeout/user-agent policy, and no React-side secrets.
   - Preview the target URL/method and any redacted headers before apply.
   - Reject unsupported schemes, unsafe/private targets if policy forbids them,
     missing URLs, invalid header/body JSON, or credential material that would be
     stored on the React side.
   - Do not hide upstream credentials or rate-limit requirements behind fake
     success. If a source needs credentials, return a typed remediation state.

5. Integrate Playground and templates.
   - Add a one-step path from a successful Playground HTTP/MCP run to a saved
     datasource-backed widget proposal.
   - If the user chooses "Use as widget" or Build Chat uses Playground context,
     carry the source config/sample shape as typed metadata, not pasted prose.
   - Existing templates that mention `http_request` should produce
     materialized HTTP datasources on apply.

6. Surface provenance and impact.
   - Proposal preview groups widgets by resolved datasource action.
   - Applied widgets record `datasource_definition_id`, binding source,
     `bound_at`, output mapping, and tail pipeline.
   - Workbench consumer rows should immediately show Build-created datasources
     without an app restart or manual rescan.
   - Dashboard history/version diffs should distinguish "new datasource created"
     from "widget config changed".

7. Add regression coverage for no-manual-setup behavior.
   - Cover Build apply creating a new HTTP datasource.
   - Cover Build apply creating a new MCP datasource.
   - Cover two widgets sharing one new HTTP datasource with distinct tail
     pipelines.
   - Cover reuse of an existing matching datasource.
   - Cover JSON argument canonicalization dedupe.
   - Cover validation rejecting unsafe/unsupported HTTP sources.
   - Cover Playground successful run to datasource-backed widget.

## Files

- `src-tauri/src/models/datasource.rs`
- `src-tauri/src/models/dashboard.rs`
- `src-tauri/src/models/widget.rs`
- `src-tauri/src/models/validation.rs`
- `src-tauri/src/modules/storage.rs`
- `src-tauri/src/modules/tool_engine.rs`
- `src-tauri/src/modules/workflow_engine.rs`
- `src-tauri/src/commands/dashboard.rs`
- `src-tauri/src/commands/datasource.rs`
- `src-tauri/src/commands/validation.rs`
- `src-tauri/src/commands/chat.rs`
- `src/lib/api.ts`
- `src/lib/templates/index.ts`
- `src/components/layout/ChatPanel.tsx`
- `src/components/layout/DashboardGrid.tsx`
- `src/components/datasource/Workbench.tsx`
- `src/components/playground/Playground.tsx`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W39_AUTOMATIC_DATASOURCE_MATERIALIZATION.md`

## Validation

- `node -e "JSON.parse(require('fs').readFileSync('src-tauri/tauri.conf.json','utf8'))"`
- `bun run check:contract`
- `bun run typecheck`
- `bun run build`
- `cargo fmt --all --check` or targeted `rustfmt --edition 2021` for changed
  Rust files if unrelated format drift exists.
- `cargo check --workspace --all-targets`
- Unit or integration checks for:
  - canonical signature equality for reordered JSON arguments,
  - Build apply creating a saved HTTP datasource,
  - Build apply creating a saved MCP datasource,
  - shared proposal source materialized once for multiple widgets,
  - inline duplicate datasource plans collapsed to one saved definition,
  - existing saved datasource reuse with per-widget tail pipelines,
  - unsupported HTTP source returning a typed validation issue,
  - Playground run metadata producing a saved datasource-backed widget,
  - Workbench consumer lookup after automatic materialization,
  - export/import preserving automatically materialized datasource identity.
- Manual running-app smoke:
  - start with an empty profile and no saved datasources,
  - ask Build Chat to create a dashboard backed by a public HTTP JSON endpoint,
  - confirm preview shows a datasource create action and requires explicit apply,
  - apply and confirm Workbench now lists the datasource automatically,
  - add a second widget over the same endpoint and confirm it reuses the saved
    datasource instead of creating a duplicate,
  - create a Playground HTTP run, use it as a widget, and confirm no manual
    Workbench setup was required,
  - reload the app and confirm widgets still refresh through saved datasource
    bindings.

## Out of scope

- Auto-applying Build Chat proposals without user confirmation.
- Replacing the workflow engine or PipelineStep DSL.
- Arbitrary JavaScript datasource execution.
- Bypassing W29 no-fake-success provider/tool validation.
- Storing provider/API secrets in React state, chat prompt text, or widget JSON.
- Solving external-source marketplace/catalog enablement beyond the W37
  connector catalog.
- Team/RBAC/cloud datasource provisioning.

## Related

- `AGENTS.md`
- `docs/RECONCILIATION_PLAN.md`
- `docs/W13_DURABLE_REAL_RUNTIME_PIPELINE.md`
- `docs/W16_PROPOSAL_VALIDATION_GATE.md`
- `docs/W20_DATA_PLAYGROUND_TEMPLATES.md`
- `docs/W29_REAL_PROVIDER_RUNTIME_GATE.md`
- `docs/W30_DATASOURCE_PIPELINE_WORKBENCH.md`
- `docs/W31_DATASOURCE_IDENTITY_BINDING_PROVENANCE.md`
- `docs/W32_TYPED_PIPELINE_STUDIO.md`
- `docs/W37_EXTERNAL_OPEN_SOURCE_CATALOG.md`
- `docs/W38_BUILD_CHAT_WIDGET_MENTIONS.md`

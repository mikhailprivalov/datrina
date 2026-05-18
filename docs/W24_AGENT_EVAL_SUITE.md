# W24 Agent Eval Suite

Status: shipped v1 (replay-mode assertions, 2026-05-17). v2 deferrals
closed by W33 — see `docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md`
for the `AIProvider` trait, recorded-replay harness, live-mode lane,
and acceptance runner.

Date: 2026-05-16 (v1 landed 2026-05-17)

## v1 — what shipped

* `src-tauri/tests/agent_eval.rs` — single integration-test binary that
  loads every `tests/fixtures/agent_evals/*.yaml` scenario and runs its
  assertions against the production assertion surfaces in-process.
  Runs in under a second, no network, no provider key required.
* Six seed scenarios covering: a happy-path single stat over MCP, a
  table over HTTP, a multi-widget shared-source dashboard, a hardcoded-
  literal regression, a text-widget-as-JSON regression, and a captured
  looping tool-call sequence.
* Ten assertion kinds: `validator_passes`, `validator_fails_with`,
  `no_hardcoded_literals`, `proposal_widget` (kind/count/datasource
  presence/pipeline step kinds), `tool_called`, `plan_step_kind`,
  `plan_step_count`, `loop_detected`, `cost_lt_usd`.
* `expensive_evals` feature flag wired in Cargo.toml. The live test
  body is a documented `#[ignore]` placeholder pointing at the v2
  prerequisites — kept honest rather than stubbed silently.
* Two fault-injection self-tests in `agent_eval.rs` that feed synthetic
  bad proposals into the assertion library and confirm the assertions
  actually trip; protects against the assertions silently rotting into
  no-ops after a future validator refactor.
* `bun run eval` shortcut; README "Running agent evals" section.

The runner mirrors `chat.rs::count_recent_repeats` /
`canonical_json_string` in the test file rather than re-exporting them
from the production binary. Intentional: if a future change to the
runtime heuristic diverges from the mirror, the affected scenario
fails with a clear diff, which is exactly the regression signal the
suite exists to provide.

## v2 status (closed by W33)

W33 extracted the `AIProvider` trait, added a `RecordedProvider` that
implements it, and wired a deterministic full-loop replay harness +
`replay_loop_passes` assertion alongside the existing v1 assertions.
The `--features expensive_evals` live lane is now a real
credentials-gated test, not a placeholder. See
`docs/W33_REAL_PROVIDER_ACCEPTANCE_AND_AGENT_EVAL_V2.md` for the
shipped surfaces (trait, recorded provider, capability map,
acceptance runner).

Still deferred:

* Recorded SSE chunk replay (the `record_eval` example) — the W33
  harness uses non-streaming `complete()` only.
* HTML drift report (`target/eval_report.html`) — the W33 acceptance
  runner writes JSON; HTML rendering is downstream.

## Original plan

The sections below are the v0 plan as written 2026-05-16. Kept as
reference for the v2 work; some terminology (recording capture, MockProvider) is
v2 scope.

## Context

There are no automated tests for the agent. `find src-tauri -path
"*test*"` is empty. Every system-prompt change, every new tool, every
pipeline refactor is verified by hand against one or two prompts the
user happens to remember. Regressions land silently. The eval surface
needs to exist before W16/W17/W18 land in production, because each of
those will introduce new prompt and tool-loop logic that must not
regress.

## Goal

A reproducible, fast-by-default eval suite that runs against the
production agent pipeline. Two modes:

- **Recorded mode** (default, `cargo test`): provider responses replayed
  from on-disk fixtures. Deterministic, free, runs in CI in seconds.
- **Live mode** (`cargo test --features expensive_evals`): hits real
  OpenRouter with the configured model. Used before releases and after
  prompt changes to gauge real model behavior.

Each eval is a structured assertion set against the agent's full output
(proposal, tool calls, plan, validator result), not just a single field.

## Approach

### Fixture format

`src-tauri/tests/fixtures/agent_evals/<scenario>.yaml`:

```yaml
id: release_status_single_stat
description: User asks for a single-stat dashboard summarising the active
  release of a user-configured project via a configured stdio MCP server.
prompt: |
  Build a dashboard with a stat widget showing the currently active
  release for project "<example-project>". Use the configured stdio MCP
  server "<example-mcp>".

setup:
  mcp_servers:
    - id: example-mcp
      kind: stdio
      command: mocked
      tools:
        - name: list_releases
          input_schema: { type: object, properties: { project: { type: string } } }

provider:
  kind: openai_compatible
  model: <configured-model>
  recording: release_status_single_stat.recording.jsonl

assertions:
  - kind: tool_called
    tool: submit_plan
    position: first
  - kind: tool_called
    tool: mcp_tool
    args_path: tool_name
    args_equals: list_releases
  - kind: tool_called
    tool: dry_run_widget
    min_count: 1
  - kind: validator_passes
  - kind: proposal_widget
    count: 1
    kind: stat
    has_datasource_plan: true
    has_pipeline_step_kind: ["pluck", "filter"]
  - kind: no_hardcoded_literals
  - kind: no_loop_detected
  - kind: cost_lt_usd
    value: 0.20
```

### Recording capture

A helper command `cargo run --example record_eval -- <scenario.yaml>`:

1. Runs the eval scenario against the real provider once.
2. Captures every SSE chunk, every tool result, every timing into the
   `recording.jsonl` file.
3. After capture, runs the assertions against the captured run to make
   sure the recording is itself valid evidence.

Recordings are committed to git. Re-recording is explicit — never
happens silently in CI.

### Replay mode (default test path)

`src-tauri/tests/agent_eval.rs`:

```rust
#[tokio::test]
async fn release_status_single_stat() {
    let scenario = load_scenario("release_status_single_stat");
    let provider = MockProvider::from_recording(&scenario.provider.recording);
    let mcp = MockMcpServer::from_setup(&scenario.setup.mcp_servers);
    let outcome = run_scenario(&scenario, provider, mcp).await.unwrap();
    assert_scenario(&scenario.assertions, &outcome);
}
```

`MockProvider`:

- Implements the same trait as `AIEngine` consumers (extract an
  `AIProvider` trait if it doesn't exist yet).
- Returns chunks from the recording in the exact order, respecting
  tool_call boundaries.
- If the agent's prompt diverges from the recording's prompt by more
  than N tokens, the test fails with a clear "recording out of date"
  message — prevents silent drift after prompt edits.

`MockMcpServer`:

- Speaks the same JSON-RPC protocol the production `MCPManager` uses.
- Tool responses configured per-scenario in the YAML.

### Live mode

`cargo test --features expensive_evals` flips a feature flag that
swaps `MockProvider` for the real provider. Requires `OPENROUTER_API_KEY`
in env; tests `skip` with a clear message otherwise. Assertions are the
same — providers may produce different exact text but should still
satisfy structural assertions.

Cost guard: live mode aggregates total cost across the suite; aborts
if `>$1.00` total to prevent runaway.

### Assertion library

`src-tauri/tests/eval_assertions.rs` — implementations of the assertion
kinds. Pluggable; new kinds added by extending the enum and adding a
match arm. Initial set:

- `tool_called` (name, position/order, optional args matcher).
- `min_iterations`, `max_iterations`.
- `validator_passes` / `validator_fails_with` (uses W16 validator).
- `proposal_widget` (kind, count, optional datasource_plan / pipeline checks).
- `no_hardcoded_literals` (uses W16 validator).
- `no_loop_detected`.
- `cost_lt_usd` (uses W22 usage tracking).
- `plan_step_count`, `plan_step_kind` (uses W18 plan artifact).

### Initial scenario set

Seed with 6 scenarios covering known failure modes:

1. `release_status_single_stat` — MCP stdio + single stat widget,
   end-to-end happy path.
2. `crypto_top10_table` — no MCP, http_request-only path.
3. `system_monitor_grouped` — multiple widgets, shared datasource.
4. `text_widget_no_json_dump` — explicit test that text widgets are
   markdown, never JSON dumps.
5. `looping_tool_call` — pathological prompt that historically caused
   loops; assert loop_detected fires and run terminates.
6. `bad_pipeline_fails_validator` — agent emits a widget with no
   datasource_plan; assert validator catches and forces retry.

### CI hook

GitHub Actions workflow (or local `bun run eval` script if no CI yet):

```yaml
- name: Run agent eval suite (replay mode)
  run: cargo test --test agent_eval --release
```

Replay mode runs in <30 seconds. Live mode is documented but not in CI;
run manually before releases.

### Reporting

After every run, write `target/eval_report.html` with:

- Per-scenario pass/fail.
- Per-assertion details.
- For live runs, per-scenario cost and latency.
- Diff between replay and (cached) prior live run, surfacing drift.

## Files to touch

- `src-tauri/Cargo.toml` — `dev-dependencies` for `tokio-test`,
  `serde_yaml`, `pretty_assertions`; new feature `expensive_evals`.
- `src-tauri/tests/agent_eval.rs` (new).
- `src-tauri/tests/eval_assertions.rs` (new).
- `src-tauri/tests/eval_runner.rs` (new) — scenario loader, mocks.
- `src-tauri/tests/fixtures/agent_evals/*.yaml` (new) — scenarios.
- `src-tauri/tests/fixtures/agent_evals/recordings/*.jsonl` (new).
- `src-tauri/examples/record_eval.rs` (new).
- `src-tauri/src/modules/ai.rs` — extract `AIProvider` trait so it can
  be mocked.
- `src-tauri/src/modules/mcp_manager.rs` — likewise, an `McpServer`
  trait for mocking the stdio side.
- `README.md` — section "Running agent evals".
- `.github/workflows/agent-eval.yml` (if CI exists).
- `package.json` — `"eval": "cargo test --test agent_eval --release"`.

## Validation

- `cargo test --test agent_eval` runs in < 30 s on a clean checkout, all
  6 scenarios pass.
- `cargo test --features expensive_evals --test agent_eval` against a
  real OpenRouter key passes for the same 6 scenarios.
- Mutate the system prompt to remove the anti-hardcode rule; run replay
  mode; the `no_hardcoded_literals` assertion fails as expected.
- Mutate the validator to always return OK; the
  `bad_pipeline_fails_validator` scenario fails as expected.

## Out of scope

- Property-based prompt testing.
- Multi-turn conversation evals (single-turn only in v1).
- Cross-model parity assertions.
- Performance benchmarks (correctness only).

## Related

- W16 — validator is a primary assertion surface.
- W17 — memory retrieval can be seeded in test fixtures so memory-aware
  behavior is asserted reproducibly.
- W18 — plan + reflection assertions live here.
- W22 — cost assertions depend on the usage parsing being live.

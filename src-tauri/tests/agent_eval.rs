//! W24 — agent eval suite (replay mode).
//! W33 — adds the `AIProvider` trait + `RecordedProvider` replay harness.
//!
//! This binary loads every YAML scenario under
//! `tests/fixtures/agent_evals/`, materialises the captured agent output
//! (proposal, tool-call history, plan, token usage), and runs the
//! scenario's assertions against the production assertion surfaces:
//!
//!   * `validate_build_proposal` — W16 deterministic gate.
//!   * `pricing_for` + `ModelPricing::cost_for` — W22 cost math.
//!   * A re-implemented `count_recent_repeats` mirror for loop-detection
//!     coverage. Mirrors the runtime heuristic in
//!     `src-tauri/src/commands/chat.rs` deliberately so a divergence
//!     surfaces here.
//!   * W33: the new `replay_loop_passes` assertion drives a captured
//!     `RecordedProvider` (impl `AIProvider`) through a small
//!     turn-by-turn loop that mirrors the real agent's flow — submit
//!     plan, dry-run, validate, emit proposal — and asserts the same
//!     validator/cost gates against the final state. Streaming chunks
//!     are *not* recorded; non-streaming `complete()` is sufficient
//!     because the validator/cost surfaces never look at the SSE
//!     transport, only the resolved tool calls + final content.
//!
//! Scenarios are committed snapshots: they encode the *shape* of a real
//! agent run, not a live re-execution. The live-provider lane (gated by
//! `--features expensive_evals`) is a separate `#[ignore]`d entry point
//! at the bottom of this file; it reads `DATRINA_LIVE_*` env vars and
//! runs against a real LLM when explicitly requested.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value as JsonValue;

use datrina_lib::commands::validation::{
    parse_build_proposal_content, validate_build_proposal, validate_build_proposal_full,
    MentionedSource,
};
use datrina_lib::models::chat::{
    ChatMessage, ChatMessagePart, ChatMode, MessageRole, PlanStep, PlanStepKind, TokenUsage,
    ToolCall, ToolResult,
};
use datrina_lib::models::dashboard::{BuildProposal, BuildWidgetType};
use datrina_lib::models::pricing::{pricing_for, ModelPricing, UsageReport};
use datrina_lib::models::provider::{
    supports_structured_output, LLMProvider, ProviderKind, StructuredOutputCapability,
};
use datrina_lib::models::validation::ValidationIssue;
use datrina_lib::modules::ai::{AIProvider, AIResponse, AIToolSpec};

// ─── YAML schema ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Scenario {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    // surfaced via {:?} in panic reports and to scenario authors reading YAML
    description: String,
    trace: ScenarioTrace,
    assertions: Vec<Assertion>,
    /// W48: source mentions that should be considered "named by the
    /// user" when running validator gates. Each entry must include a
    /// label and at least one identifier (`datasource_definition_id`
    /// or `workflow_id`).
    #[serde(default)]
    mentioned_sources: Vec<ScenarioMentionedSource>,
}

#[derive(Debug, Deserialize)]
struct ScenarioMentionedSource {
    label: String,
    #[serde(default)]
    datasource_definition_id: Option<String>,
    #[serde(default)]
    workflow_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScenarioTrace {
    proposal: BuildProposal,
    #[serde(default)]
    tool_calls: Vec<ScenarioToolCall>,
    #[serde(default)]
    plan: Option<ScenarioPlan>,
    #[serde(default)]
    usage: Option<ScenarioUsage>,
    /// W33: optional recorded provider turns. When present, the
    /// `replay_loop_passes` assertion drives a full agent loop through
    /// `RecordedProvider` and re-asserts the same validator/cost gates
    /// against the resulting final state.
    #[serde(default)]
    turns: Vec<ScenarioTurn>,
}

/// W33: one captured provider turn. The recorded provider returns
/// these in order, one per `complete()` call. Each turn carries either
/// final assistant content (the proposal JSON), tool calls (the agent
/// asking the harness to execute something), or both — matching the
/// shape of a real assistant response.
#[derive(Debug, Deserialize)]
struct ScenarioTurn {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<ScenarioToolCall>,
    #[serde(default)]
    tokens: Option<ScenarioUsage>,
    /// W33: per-turn requested structured-output mode. Optional;
    /// defaults to `PlainText` for non-final turns and `JsonObject` for
    /// the turn that emits the final proposal so the recorded provider
    /// surfaces the resolved fallback like production does.
    #[serde(default)]
    request_strict_mode: bool,
}

/// Wire-format PlanArtifact for YAML scenarios. The runtime PlanArtifact
/// carries a `created_at` timestamp that scenario authors should not have
/// to fill in by hand; we drop it here so fixtures stay terse.
#[derive(Debug, Deserialize)]
struct ScenarioPlan {
    #[serde(default)]
    #[allow(dead_code)] // kept so YAML scenarios can document plan intent inline
    summary: String,
    #[serde(default)]
    steps: Vec<PlanStep>,
}

#[derive(Debug, Deserialize)]
struct ScenarioToolCall {
    call_id: Option<String>,
    name: String,
    #[serde(default)]
    arguments: JsonValue,
    #[serde(default)]
    result: JsonValue,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ScenarioUsage {
    provider_kind: ProviderKind,
    model: String,
    prompt: u32,
    completion: u32,
    #[serde(default)]
    reasoning: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Assertion {
    /// W16 validator returns zero issues against the captured proposal +
    /// dry-run history.
    ValidatorPasses,
    /// W16 validator returns at least one issue of the named variant.
    ValidatorFailsWith { issue: String },
    /// Convenience subset of `validator_passes` that focuses on the
    /// HardcodedLiteralValue variant — kept separate so it stays callable
    /// in scenarios that are *also* expected to fail other gates.
    NoHardcodedLiterals,
    /// Structural assertion against `proposal.widgets`.
    ProposalWidget {
        #[serde(default)]
        widget_type: Option<String>,
        #[serde(default)]
        count: Option<usize>,
        #[serde(default)]
        has_datasource_plan: Option<bool>,
        #[serde(default)]
        has_pipeline_step_kind: Option<Vec<String>>,
    },
    /// At least `min_count` captured tool calls named `name`.
    ToolCalled {
        name: String,
        #[serde(default = "default_min_count")]
        min_count: usize,
    },
    /// Plan must contain at least one step for each named kind.
    PlanStepKind { kinds: Vec<String> },
    /// Plan must contain >= `min` steps.
    PlanStepCount { min: usize },
    /// Loop-detection mirror — passes when the captured tool history
    /// contains >= `min_repeats` calls of `name` with byte-identical
    /// canonical arguments. Confirms the runtime heuristic still trips
    /// on this scenario.
    LoopDetected {
        name: String,
        #[serde(default = "default_min_repeats")]
        min_repeats: usize,
    },
    /// Cost gate — uses the same pricing table as the chat path.
    CostLtUsd { value: f64 },
    /// W33: drive the captured `turns` through a `RecordedProvider` and
    /// assert the final state passes the validator and stays under the
    /// optional cost ceiling. Fails when `turns` is empty.
    ReplayLoopPasses {
        #[serde(default)]
        max_cost_usd: Option<f64>,
        /// When set, the resolved structured-output mode for the final
        /// proposal-emitting turn must equal this value. Lets the YAML
        /// pin "strict mode was actually applied" as evidence vs. a
        /// visible fallback.
        #[serde(default)]
        expect_strict_mode: Option<bool>,
    },
    /// W48: validator must not raise `UnusedSourceMention` when the
    /// scenario's `mentioned_sources` are checked against the captured
    /// proposal.
    SourceMentionsCovered,
    /// W48: the inverse — validator MUST raise `UnusedSourceMention`
    /// and the missing entry list must contain every id in
    /// `expect_missing` (the test scenario authors should set these to
    /// the `datasource_definition_id` of every source the agent
    /// silently dropped).
    SourceMentionsMissing { expect_missing: Vec<String> },
}

fn default_min_count() -> usize {
    1
}
fn default_min_repeats() -> usize {
    3
}

// ─── Loader + entrypoint ────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("agent_evals")
}

fn load_scenarios(dir: &Path) -> Vec<(PathBuf, Scenario)> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read fixtures dir {}: {}", dir.display(), e));
    for entry in entries {
        let entry = entry.expect("read_dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e));
        let scenario: Scenario = serde_yaml::from_str(&raw)
            .unwrap_or_else(|e| panic!("yaml parse failed for {}: {}", path.display(), e));
        out.push((path, scenario));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        !out.is_empty(),
        "no eval scenarios found under {}",
        dir.display()
    );
    out
}

#[test]
fn agent_evals_seed_scenarios_pass() {
    let scenarios = load_scenarios(&fixtures_dir());
    let mut report = Vec::new();
    let mut failures = 0usize;

    for (path, scenario) in &scenarios {
        let outcome = run_scenario(scenario);
        let failed: Vec<&AssertionOutcome> = outcome.iter().filter(|o| o.error.is_some()).collect();
        if failed.is_empty() {
            report.push(format!("✓ {} ({} assertions)", scenario.id, outcome.len()));
        } else {
            failures += failed.len();
            report.push(format!(
                "✗ {} — {}/{} assertions failed (file: {})",
                scenario.id,
                failed.len(),
                outcome.len(),
                path.display()
            ));
            for f in failed {
                report.push(format!(
                    "    · {} → {}",
                    f.label,
                    f.error.as_deref().unwrap_or("(no message)")
                ));
            }
        }
    }

    let summary = report.join("\n");
    println!("\nW24 agent eval suite:\n{}\n", summary);
    if failures > 0 {
        panic!(
            "{} assertion(s) failed across {} scenario(s):\n{}",
            failures,
            scenarios.len(),
            summary
        );
    }
}

// ─── Per-scenario runner ────────────────────────────────────────────────────

struct AssertionOutcome {
    label: String,
    error: Option<String>,
}

fn run_scenario(scenario: &Scenario) -> Vec<AssertionOutcome> {
    let transcript = synth_transcript(&scenario.trace);
    scenario
        .assertions
        .iter()
        .map(|assertion| {
            let label = format!("{:?}", assertion);
            let error = match assertion {
                Assertion::ValidatorPasses => assert_validator_passes(scenario, &transcript),
                Assertion::ValidatorFailsWith { issue } => {
                    assert_validator_fails_with(scenario, &transcript, issue)
                }
                Assertion::NoHardcodedLiterals => {
                    assert_no_hardcoded_literals(scenario, &transcript)
                }
                Assertion::ProposalWidget {
                    widget_type,
                    count,
                    has_datasource_plan,
                    has_pipeline_step_kind,
                } => assert_proposal_widget(
                    scenario,
                    widget_type.as_deref(),
                    *count,
                    *has_datasource_plan,
                    has_pipeline_step_kind.as_deref(),
                ),
                Assertion::ToolCalled { name, min_count } => {
                    assert_tool_called(scenario, name, *min_count)
                }
                Assertion::PlanStepKind { kinds } => assert_plan_step_kind(scenario, kinds),
                Assertion::PlanStepCount { min } => assert_plan_step_count(scenario, *min),
                Assertion::LoopDetected { name, min_repeats } => {
                    assert_loop_detected(scenario, name, *min_repeats)
                }
                Assertion::CostLtUsd { value } => assert_cost_lt_usd(scenario, *value),
                Assertion::ReplayLoopPasses {
                    max_cost_usd,
                    expect_strict_mode,
                } => assert_replay_loop_passes(scenario, *max_cost_usd, *expect_strict_mode),
                Assertion::SourceMentionsCovered => {
                    assert_source_mentions_covered(scenario, &transcript)
                }
                Assertion::SourceMentionsMissing { expect_missing } => {
                    assert_source_mentions_missing(scenario, &transcript, expect_missing)
                }
            };
            AssertionOutcome { label, error }
        })
        .collect()
}

// ─── Synthesise a ChatMessage transcript from the captured tool calls ───────
//
// `validate_build_proposal` expects to read assistant `tool_calls` and the
// matching `tool` role `tool_results` to compute dry-run evidence. We
// rebuild that minimal shape from the scenario YAML so the validator runs
// exactly as it would inside `send_message_stream_inner`.

fn synth_transcript(trace: &ScenarioTrace) -> Vec<ChatMessage> {
    if trace.tool_calls.is_empty() {
        return Vec::new();
    }
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();
    for (idx, call) in trace.tool_calls.iter().enumerate() {
        let call_id = call
            .call_id
            .clone()
            .unwrap_or_else(|| format!("scenario-call-{}", idx));
        tool_calls.push(ToolCall {
            id: call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
        tool_results.push(ToolResult {
            tool_call_id: call_id,
            name: call.name.clone(),
            result: call.result.clone(),
            error: call.error.clone(),
            compression: None,
        });
    }
    vec![
        ChatMessage {
            id: "scenario-assistant".to_string(),
            role: MessageRole::Assistant,
            content: String::new(),
            parts: Vec::<ChatMessagePart>::new(),
            mode: ChatMode::Build,
            tool_calls: Some(tool_calls),
            tool_results: None,
            metadata: None,
            timestamp: 0,
        },
        ChatMessage {
            id: "scenario-tool".to_string(),
            role: MessageRole::Tool,
            content: String::new(),
            parts: Vec::<ChatMessagePart>::new(),
            mode: ChatMode::Build,
            tool_calls: None,
            tool_results: Some(tool_results),
            metadata: None,
            timestamp: 0,
        },
    ]
}

// ─── Assertion implementations ──────────────────────────────────────────────

fn assert_validator_passes(scenario: &Scenario, transcript: &[ChatMessage]) -> Option<String> {
    let issues = validate_build_proposal(&scenario.trace.proposal, None, transcript, None);
    if issues.is_empty() {
        None
    } else {
        Some(format!(
            "expected zero validator issues, got {}: [{}]",
            issues.len(),
            issues
                .iter()
                .map(|i| i.summary())
                .collect::<Vec<_>>()
                .join(" | ")
        ))
    }
}

fn assert_validator_fails_with(
    scenario: &Scenario,
    transcript: &[ChatMessage],
    expected_variant: &str,
) -> Option<String> {
    // W48: when the scenario carries `mentioned_sources`, route through
    // the full validator entry point so the coverage gate is part of
    // what `validator_fails_with` can match against.
    let mentioned = scenario_mentioned_sources(scenario);
    let issues = if mentioned.is_empty() {
        validate_build_proposal(&scenario.trace.proposal, None, transcript, None)
    } else {
        validate_build_proposal_full(
            &scenario.trace.proposal,
            None,
            transcript,
            None,
            Some(mentioned.as_slice()),
        )
    };
    if issues
        .iter()
        .any(|issue| issue_variant_tag(issue) == expected_variant)
    {
        None
    } else if issues.is_empty() {
        Some(format!(
            "expected variant `{}`, validator returned no issues at all",
            expected_variant
        ))
    } else {
        Some(format!(
            "expected variant `{}`, got [{}]",
            expected_variant,
            issues
                .iter()
                .map(|i| issue_variant_tag(i).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

fn issue_variant_tag(issue: &ValidationIssue) -> &'static str {
    match issue {
        ValidationIssue::MissingDatasourcePlan { .. } => "missing_datasource_plan",
        ValidationIssue::UnknownReplaceWidgetId { .. } => "unknown_replace_widget_id",
        ValidationIssue::UnknownSourceKey { .. } => "unknown_source_key",
        ValidationIssue::HardcodedLiteralValue { .. } => "hardcoded_literal_value",
        ValidationIssue::TextWidgetContainsRawJson { .. } => "text_widget_contains_raw_json",
        ValidationIssue::MissingDryRunEvidence { .. } => "missing_dry_run_evidence",
        ValidationIssue::PipelineSchemaInvalid { .. } => "pipeline_schema_invalid",
        ValidationIssue::DuplicateSharedKey { .. } => "duplicate_shared_key",
        ValidationIssue::UnknownParameterReference { .. } => "unknown_parameter_reference",
        ValidationIssue::ParameterCycle { .. } => "parameter_cycle",
        ValidationIssue::OffTargetWidgetReplace { .. } => "off_target_widget_replace",
        ValidationIssue::OffTargetWidgetRemove { .. } => "off_target_widget_remove",
        ValidationIssue::UnsafeHttpDatasource { .. } => "unsafe_http_datasource",
        ValidationIssue::HardcodedGalleryItems { .. } => "hardcoded_gallery_items",
        ValidationIssue::ProposedExplicitCoordinates { .. } => "proposed_explicit_coordinates",
        ValidationIssue::ConflictingLayoutFields { .. } => "conflicting_layout_fields",
        ValidationIssue::UnusedSourceMention { .. } => "unused_source_mention",
    }
}

fn assert_no_hardcoded_literals(scenario: &Scenario, transcript: &[ChatMessage]) -> Option<String> {
    let issues = validate_build_proposal(&scenario.trace.proposal, None, transcript, None);
    let offenders: Vec<String> = issues
        .iter()
        .filter_map(|issue| match issue {
            ValidationIssue::HardcodedLiteralValue {
                widget_title, path, ..
            } => Some(format!("{} @ {}", widget_title, path)),
            _ => None,
        })
        .collect();
    if offenders.is_empty() {
        None
    } else {
        Some(format!(
            "hardcoded literal(s) detected: [{}]",
            offenders.join(", ")
        ))
    }
}

fn assert_proposal_widget(
    scenario: &Scenario,
    widget_type: Option<&str>,
    count: Option<usize>,
    has_datasource_plan: Option<bool>,
    has_pipeline_step_kind: Option<&[String]>,
) -> Option<String> {
    let widgets = &scenario.trace.proposal.widgets;
    let kind_filter: Option<BuildWidgetType> = widget_type.and_then(parse_widget_type);
    if widget_type.is_some() && kind_filter.is_none() {
        return Some(format!(
            "widget_type `{}` is not a known BuildWidgetType",
            widget_type.unwrap()
        ));
    }
    let matched: Vec<_> = widgets
        .iter()
        .filter(|w| match kind_filter.as_ref() {
            None => true,
            Some(k) => std::mem::discriminant(&w.widget_type) == std::mem::discriminant(k),
        })
        .collect();

    if let Some(expected) = count {
        if matched.len() != expected {
            return Some(format!(
                "expected {} widget(s) of type {:?}, got {}",
                expected,
                widget_type.unwrap_or("any"),
                matched.len()
            ));
        }
    }

    if let Some(required) = has_datasource_plan {
        for w in &matched {
            let present = w.datasource_plan.is_some();
            if present != required {
                return Some(format!(
                    "widget '{}' datasource_plan presence={} (expected {})",
                    w.title, present, required
                ));
            }
        }
    }

    if let Some(required_kinds) = has_pipeline_step_kind {
        for w in &matched {
            let plan = match w.datasource_plan.as_ref() {
                Some(p) => p,
                None => {
                    return Some(format!(
                        "widget '{}' has no datasource_plan; cannot satisfy has_pipeline_step_kind {:?}",
                        w.title, required_kinds
                    ));
                }
            };
            let kinds_in_pipeline: Vec<String> = plan
                .pipeline
                .iter()
                .map(|step| pipeline_step_kind_label(step).to_string())
                .collect();
            for required in required_kinds {
                if !kinds_in_pipeline.iter().any(|k| k == required) {
                    return Some(format!(
                        "widget '{}' pipeline missing required step kind `{}` (saw {:?})",
                        w.title, required, kinds_in_pipeline
                    ));
                }
            }
        }
    }
    None
}

fn parse_widget_type(name: &str) -> Option<BuildWidgetType> {
    match name {
        "chart" => Some(BuildWidgetType::Chart),
        "text" => Some(BuildWidgetType::Text),
        "table" => Some(BuildWidgetType::Table),
        "image" => Some(BuildWidgetType::Image),
        "gauge" => Some(BuildWidgetType::Gauge),
        "stat" => Some(BuildWidgetType::Stat),
        "logs" => Some(BuildWidgetType::Logs),
        "bar_gauge" => Some(BuildWidgetType::BarGauge),
        "status_grid" => Some(BuildWidgetType::StatusGrid),
        "heatmap" => Some(BuildWidgetType::Heatmap),
        _ => None,
    }
}

fn pipeline_step_kind_label(step: &datrina_lib::models::pipeline::PipelineStep) -> &'static str {
    use datrina_lib::models::pipeline::PipelineStep::*;
    match step {
        Pick { .. } => "pick",
        Filter { .. } => "filter",
        Sort { .. } => "sort",
        Limit { .. } => "limit",
        Map { .. } => "map",
        Aggregate { .. } => "aggregate",
        Set { .. } => "set",
        Head => "head",
        Tail => "tail",
        Length => "length",
        Flatten => "flatten",
        Unique { .. } => "unique",
        Format { .. } => "format",
        Coerce { .. } => "coerce",
        LlmPostprocess { .. } => "llm_postprocess",
        McpCall { .. } => "mcp_call",
    }
}

fn assert_tool_called(scenario: &Scenario, name: &str, min_count: usize) -> Option<String> {
    let n = scenario
        .trace
        .tool_calls
        .iter()
        .filter(|c| c.name == name)
        .count();
    if n >= min_count {
        None
    } else {
        Some(format!(
            "expected >={} call(s) to `{}`, captured {}",
            min_count, name, n
        ))
    }
}

fn assert_plan_step_kind(scenario: &Scenario, required_kinds: &[String]) -> Option<String> {
    let plan = match scenario.trace.plan.as_ref() {
        Some(p) => p,
        None => return Some("plan_step_kind: scenario has no plan".to_string()),
    };
    let actual: Vec<String> = plan
        .steps
        .iter()
        .map(|s| plan_step_kind_label(s.kind).to_string())
        .collect();
    for required in required_kinds {
        if !actual.iter().any(|k| k == required) {
            return Some(format!(
                "plan missing required step kind `{}` (saw {:?})",
                required, actual
            ));
        }
    }
    None
}

fn plan_step_kind_label(kind: PlanStepKind) -> &'static str {
    match kind {
        PlanStepKind::Explore => "explore",
        PlanStepKind::Fetch => "fetch",
        PlanStepKind::Design => "design",
        PlanStepKind::Test => "test",
        PlanStepKind::Propose => "propose",
        PlanStepKind::Other => "other",
    }
}

fn assert_plan_step_count(scenario: &Scenario, min: usize) -> Option<String> {
    let n = scenario
        .trace
        .plan
        .as_ref()
        .map(|p| p.steps.len())
        .unwrap_or(0);
    if n >= min {
        None
    } else {
        Some(format!("plan has {} step(s), expected >= {}", n, min))
    }
}

fn assert_loop_detected(scenario: &Scenario, name: &str, min_repeats: usize) -> Option<String> {
    // Mirror chat.rs::count_recent_repeats: the runtime keys on
    // `(tool_name, canonical_json_string(arguments))` and trips when the
    // same key appears >=3 times in the recent window. We replicate that
    // shape here so a future change to the heuristic surfaces as a diff.
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    let mut max_for_named = 0usize;
    for call in &scenario.trace.tool_calls {
        let key = (call.name.clone(), canonical_json_string(&call.arguments));
        let n = counts.entry(key).or_insert(0);
        *n += 1;
        if call.name == name && *n > max_for_named {
            max_for_named = *n;
        }
    }
    if max_for_named >= min_repeats {
        None
    } else {
        Some(format!(
            "expected >={} identical-arg repeats of `{}`, saw at most {}",
            min_repeats, name, max_for_named
        ))
    }
}

fn canonical_json_string(value: &JsonValue) -> String {
    let mut out = String::new();
    walk(value, &mut out);
    return out;

    fn walk(value: &JsonValue, out: &mut String) {
        match value {
            JsonValue::Null => out.push_str("null"),
            JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            JsonValue::Number(n) => out.push_str(&n.to_string()),
            JsonValue::String(s) => {
                out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s)))
            }
            JsonValue::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    walk(item, out);
                }
                out.push(']');
            }
            JsonValue::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(k).unwrap_or_else(|_| format!("\"{}\"", k)),
                    );
                    out.push(':');
                    walk(&map[*k], out);
                }
                out.push('}');
            }
        }
    }
}

fn scenario_mentioned_sources(scenario: &Scenario) -> Vec<MentionedSource> {
    scenario
        .mentioned_sources
        .iter()
        .map(|m| MentionedSource {
            label: m.label.clone(),
            datasource_definition_id: m.datasource_definition_id.clone(),
            workflow_id: m.workflow_id.clone(),
        })
        .collect()
}

fn assert_source_mentions_covered(
    scenario: &Scenario,
    transcript: &[ChatMessage],
) -> Option<String> {
    let mentioned = scenario_mentioned_sources(scenario);
    if mentioned.is_empty() {
        return Some(
            "source_mentions_covered: scenario has no `mentioned_sources` to check against"
                .to_string(),
        );
    }
    let issues = validate_build_proposal_full(
        &scenario.trace.proposal,
        None,
        transcript,
        None,
        Some(mentioned.as_slice()),
    );
    let unused = issues.iter().find_map(|issue| match issue {
        ValidationIssue::UnusedSourceMention { missing } => Some(missing.clone()),
        _ => None,
    });
    match unused {
        None => None,
        Some(missing) => {
            let labels: Vec<String> = missing
                .iter()
                .map(|entry| {
                    entry
                        .datasource_definition_id
                        .clone()
                        .or_else(|| entry.workflow_id.clone())
                        .unwrap_or_else(|| entry.label.clone())
                })
                .collect();
            Some(format!(
                "expected every mentioned source to be referenced; validator flagged {} missing: [{}]",
                labels.len(),
                labels.join(", ")
            ))
        }
    }
}

fn assert_source_mentions_missing(
    scenario: &Scenario,
    transcript: &[ChatMessage],
    expect_missing: &[String],
) -> Option<String> {
    let mentioned = scenario_mentioned_sources(scenario);
    if mentioned.is_empty() {
        return Some(
            "source_mentions_missing: scenario has no `mentioned_sources` to check against"
                .to_string(),
        );
    }
    let issues = validate_build_proposal_full(
        &scenario.trace.proposal,
        None,
        transcript,
        None,
        Some(mentioned.as_slice()),
    );
    let unused = issues.iter().find_map(|issue| match issue {
        ValidationIssue::UnusedSourceMention { missing } => Some(missing.clone()),
        _ => None,
    });
    let Some(missing) = unused else {
        return Some("expected validator to raise UnusedSourceMention but it did not".to_string());
    };
    for expected in expect_missing {
        let found = missing.iter().any(|entry| {
            entry.datasource_definition_id.as_deref() == Some(expected.as_str())
                || entry.workflow_id.as_deref() == Some(expected.as_str())
        });
        if !found {
            return Some(format!(
                "expected missing entry `{}` not in validator output (got [{}])",
                expected,
                missing
                    .iter()
                    .map(|e| e
                        .datasource_definition_id
                        .clone()
                        .or_else(|| e.workflow_id.clone())
                        .unwrap_or_else(|| e.label.clone()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    None
}

fn assert_cost_lt_usd(scenario: &Scenario, ceiling: f64) -> Option<String> {
    let usage = match scenario.trace.usage.as_ref() {
        Some(u) => u,
        None => return Some("cost_lt_usd: scenario has no usage block".to_string()),
    };
    let pricing = match pricing_for(usage.provider_kind, &usage.model, &[]) {
        Some(p) => p,
        None => {
            // Unknown model = $0 cost in the runtime; assertion is satisfied
            // vacuously but we surface it so future model bumps don't
            // silently bypass the gate.
            eprintln!(
                "[{}] cost_lt_usd: no pricing for {}/{} — skipping (chat path would record $0)",
                scenario.id,
                provider_kind_label(usage.provider_kind),
                usage.model
            );
            return None;
        }
    };
    let report = UsageReport::new(usage.prompt, usage.completion, usage.reasoning);
    let cost = pricing.cost_for(&report);
    if cost < ceiling {
        None
    } else {
        Some(format!(
            "captured cost ${:.6} >= ceiling ${:.6} ({} / {} @ {}p+{}c{}r)",
            cost,
            ceiling,
            provider_kind_label(usage.provider_kind),
            usage.model,
            usage.prompt,
            usage.completion,
            usage.reasoning.unwrap_or(0),
        ))
    }
}

fn provider_kind_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Openrouter => "openrouter",
        ProviderKind::Ollama => "ollama",
        ProviderKind::Custom => "custom",
    }
}

// ─── W33: recorded provider + full-loop replay harness ─────────────────────

/// Captured-turn provider that satisfies `AIProvider`. The harness
/// constructs one per scenario, feeds it the YAML-declared `turns`,
/// and the harness loop consumes them one per `complete()` call. The
/// resolved structured-output mode is reported back so the assertion
/// can distinguish strict-mode application from a visible fallback.
struct RecordedProvider {
    turns: Vec<ScenarioTurn>,
    cursor: AtomicUsize,
}

impl RecordedProvider {
    fn new(turns: Vec<ScenarioTurn>) -> Self {
        Self {
            turns,
            cursor: AtomicUsize::new(0),
        }
    }

    fn remaining(&self) -> usize {
        self.turns
            .len()
            .saturating_sub(self.cursor.load(Ordering::SeqCst))
    }
}

#[async_trait]
impl AIProvider for RecordedProvider {
    async fn complete(
        &self,
        provider: &LLMProvider,
        _messages: &[ChatMessage],
        _tools: &[AIToolSpec],
        structured_output: StructuredOutputCapability,
    ) -> anyhow::Result<AIResponse> {
        let idx = self.cursor.fetch_add(1, Ordering::SeqCst);
        let turn = self.turns.get(idx).ok_or_else(|| {
            anyhow::anyhow!(
                "recorded provider exhausted: requested turn {} but only {} captured",
                idx,
                self.turns.len()
            )
        })?;
        let resolved = match structured_output {
            StructuredOutputCapability::PlainText => StructuredOutputCapability::PlainText,
            StructuredOutputCapability::JsonObject => {
                supports_structured_output(provider.kind, &provider.default_model)
            }
        };
        let tool_calls = turn
            .tool_calls
            .iter()
            .enumerate()
            .map(|(i, call)| ToolCall {
                id: call
                    .call_id
                    .clone()
                    .unwrap_or_else(|| format!("recorded-{}-{}", idx, i)),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            })
            .collect();
        let tokens = turn.tokens.as_ref().map(|u| TokenUsage {
            prompt: u.prompt,
            completion: u.completion,
            reasoning: u.reasoning,
            provider_cost_usd: None,
        });
        Ok(AIResponse {
            content: turn.content.clone(),
            provider_id: provider.id.clone(),
            model: provider.default_model.clone(),
            tokens,
            latency_ms: 0,
            tool_calls,
            reasoning: None,
            strict_mode: resolved,
        })
    }
}

/// Outcome of a full-loop replay. Mirrors what an acceptance report
/// needs from one scenario: the resolved validator outcome, the cost
/// computed from the captured usage, the structured-output mode the
/// final turn actually used, and the tool-call history that ran.
struct ReplayOutcome {
    proposal: Option<BuildProposal>,
    transcript: Vec<ChatMessage>,
    final_strict_mode: StructuredOutputCapability,
    cost_usd: f64,
    tool_call_count: usize,
    proposal_turn_idx: Option<usize>,
}

/// Drive the recorded provider through a deterministic agent loop. The
/// harness mirrors the production flow that matters for the validator
/// gate: turn-by-turn, execute every tool call the recorded provider
/// emits via a minimal in-test tool registry, then on the first turn
/// whose content parses as a `BuildProposal` JSON, stop and surface
/// the outcome.
async fn run_replay_loop(
    provider_kind: ProviderKind,
    provider_model: &str,
    turns: Vec<ScenarioTurn>,
) -> anyhow::Result<ReplayOutcome> {
    let provider = LLMProvider {
        id: "recorded-provider".into(),
        name: "Recorded".into(),
        kind: provider_kind,
        base_url: "http://recorded.invalid".into(),
        api_key: None,
        default_model: provider_model.to_string(),
        models: vec![provider_model.to_string()],
        is_enabled: true,
        is_unsupported: false,
    };
    let proposal_turn = turns
        .iter()
        .position(|turn| parse_build_proposal_content(&turn.content).is_some());
    let strict_request_idx = turns.iter().position(|t| t.request_strict_mode);
    let recorded = RecordedProvider::new(turns);

    let mut transcript: Vec<ChatMessage> = Vec::new();
    let tool_specs: Vec<AIToolSpec> = Vec::new();
    let mut proposal: Option<BuildProposal> = None;
    let mut final_strict_mode = StructuredOutputCapability::PlainText;
    let mut cost_usd = 0.0;
    let mut tool_call_count = 0usize;
    let max_turns: usize = recorded.turns.len() + 4;

    for turn_idx in 0..max_turns {
        if recorded.remaining() == 0 {
            break;
        }
        let request_mode = if strict_request_idx == Some(turn_idx) {
            StructuredOutputCapability::JsonObject
        } else {
            StructuredOutputCapability::PlainText
        };
        let response = recorded
            .complete(&provider, &transcript, &tool_specs, request_mode)
            .await?;
        final_strict_mode = response.strict_mode;
        if let Some(usage) = response.tokens.as_ref() {
            if let Some(pricing) = pricing_for(provider.kind, &response.model, &[]) {
                let report = UsageReport::new(usage.prompt, usage.completion, usage.reasoning);
                cost_usd += pricing.cost_for(&report);
            }
        }

        // Persist assistant turn (with tool_calls if any).
        let assistant = ChatMessage {
            id: format!("replay-assistant-{}", turn_idx),
            role: MessageRole::Assistant,
            content: response.content.clone(),
            parts: Vec::<ChatMessagePart>::new(),
            mode: ChatMode::Build,
            tool_calls: if response.tool_calls.is_empty() {
                None
            } else {
                Some(response.tool_calls.clone())
            },
            tool_results: None,
            metadata: None,
            timestamp: turn_idx as i64,
        };
        transcript.push(assistant);

        // Execute every tool call deterministically.
        if !response.tool_calls.is_empty() {
            let mut tool_results = Vec::with_capacity(response.tool_calls.len());
            for call in &response.tool_calls {
                tool_call_count += 1;
                let result = execute_recorded_tool(call);
                tool_results.push(result);
            }
            transcript.push(ChatMessage {
                id: format!("replay-tool-{}", turn_idx),
                role: MessageRole::Tool,
                content: String::new(),
                parts: Vec::<ChatMessagePart>::new(),
                mode: ChatMode::Build,
                tool_calls: None,
                tool_results: Some(tool_results),
                metadata: None,
                timestamp: turn_idx as i64,
            });
        }

        if let Some(parsed) = parse_build_proposal_content(&response.content) {
            proposal = Some(parsed);
            break;
        }
    }

    Ok(ReplayOutcome {
        proposal,
        transcript,
        final_strict_mode,
        cost_usd,
        tool_call_count,
        proposal_turn_idx: proposal_turn,
    })
}

/// In-test tool registry. The agent_eval scenarios cover three tools:
/// `submit_plan`, `dry_run_widget`, and a generic `mcp_call` fallback.
/// All three return synthetic deterministic results — the validator
/// only cares about presence (for `submit_plan`/`dry_run_widget`) and
/// shape, not content fidelity.
fn execute_recorded_tool(call: &ToolCall) -> ToolResult {
    match call.name.as_str() {
        "submit_plan" => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result: serde_json::json!({ "status": "plan_accepted" }),
            error: None,
            compression: None,
        },
        "dry_run_widget" => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result: serde_json::json!({ "status": "ok", "rows": 1 }),
            error: None,
            compression: None,
        },
        _ => ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            result: serde_json::json!({ "status": "ok" }),
            error: None,
            compression: None,
        },
    }
}

fn assert_replay_loop_passes(
    scenario: &Scenario,
    max_cost_usd: Option<f64>,
    expect_strict_mode: Option<bool>,
) -> Option<String> {
    if scenario.trace.turns.is_empty() {
        return Some("replay_loop_passes: scenario has no `turns` block".to_string());
    }
    let usage = scenario.trace.usage.as_ref();
    let (provider_kind, model) = match usage {
        Some(u) => (u.provider_kind, u.model.clone()),
        None => (
            ProviderKind::Openrouter,
            "moonshotai/kimi-k2.6-instruct".to_string(),
        ),
    };
    let turns = scenario.trace.turns.iter().map(clone_turn).collect();
    let outcome = match futures_executor::block_on(run_replay_loop(provider_kind, &model, turns)) {
        Ok(outcome) => outcome,
        Err(e) => return Some(format!("replay_loop_passes: harness error: {}", e)),
    };

    let proposal = match outcome.proposal.as_ref() {
        Some(p) => p,
        None => {
            return Some(format!(
                "replay_loop_passes: no turn produced a parseable BuildProposal (turn idx={:?}, transcript len={})",
                outcome.proposal_turn_idx,
                outcome.transcript.len()
            ));
        }
    };
    let issues = validate_build_proposal(proposal, None, &outcome.transcript, None);
    if !issues.is_empty() {
        return Some(format!(
            "replay_loop_passes: validator rejected the replayed proposal: [{}]",
            issues
                .iter()
                .map(|i| i.summary())
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    if let Some(ceiling) = max_cost_usd {
        if outcome.cost_usd >= ceiling {
            return Some(format!(
                "replay_loop_passes: cost ${:.6} >= ceiling ${:.6}",
                outcome.cost_usd, ceiling
            ));
        }
    }
    if let Some(expected) = expect_strict_mode {
        let actual = outcome.final_strict_mode.is_strict();
        if actual != expected {
            return Some(format!(
                "replay_loop_passes: expected strict_mode={}, actual={} (resolved={:?})",
                expected, actual, outcome.final_strict_mode
            ));
        }
    }
    if outcome.tool_call_count == 0 {
        return Some(
            "replay_loop_passes: no tool calls were executed — recorded loop is degenerate"
                .to_string(),
        );
    }
    None
}

fn clone_turn(turn: &ScenarioTurn) -> ScenarioTurn {
    ScenarioTurn {
        content: turn.content.clone(),
        tool_calls: turn
            .tool_calls
            .iter()
            .map(|c| ScenarioToolCall {
                call_id: c.call_id.clone(),
                name: c.name.clone(),
                arguments: c.arguments.clone(),
                result: c.result.clone(),
                error: c.error.clone(),
            })
            .collect(),
        tokens: turn.tokens.as_ref().map(|u| ScenarioUsage {
            provider_kind: u.provider_kind,
            model: u.model.clone(),
            prompt: u.prompt,
            completion: u.completion,
            reasoning: u.reasoning,
        }),
        request_strict_mode: turn.request_strict_mode,
    }
}

mod futures_executor {
    /// Tiny block_on so the synchronous assertion runner can drive an
    /// async harness without dragging tokio into the eval binary just
    /// for one test loop. The harness performs no I/O — every await
    /// returns immediately — so a single-threaded park-and-resume
    /// executor is sufficient.
    pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};
        use std::thread;

        struct ThreadWaker(thread::Thread);
        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }

        let waker = Waker::from(Arc::new(ThreadWaker(thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            match Pin::new(&mut future).as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => thread::park(),
            }
        }
    }
}

// ─── Live mode lane ─────────────────────────────────────────────────────────
//
// Gated behind `--features expensive_evals`. Reads provider credentials
// from environment variables and runs the replay harness against a real
// OpenAI-compatible endpoint, using a minimal one-shot prompt. Marked
// `#[ignore]` so `cargo test` does not invoke it implicitly; CI / a
// human invokes it via
//   `cargo test --features expensive_evals --test agent_eval -- --ignored`.
//
// Environment variables consumed:
//   DATRINA_LIVE_BASE_URL   — e.g. https://openrouter.ai/api
//   DATRINA_LIVE_API_KEY    — provider API key
//   DATRINA_LIVE_MODEL      — model id, e.g. openai/gpt-4o-mini
//   DATRINA_LIVE_KIND       — openrouter | ollama | custom (default openrouter)
//   DATRINA_LIVE_PROMPT     — optional system prompt override
//
// Missing/invalid env vars cause an *explicit panic* with a remediation
// message. The lane never silently no-ops, so anyone running the
// `--ignored` test sees exactly what's missing.

#[cfg(feature = "expensive_evals")]
#[test]
#[ignore = "live-mode eval — set DATRINA_LIVE_BASE_URL/API_KEY/MODEL and run with --features expensive_evals -- --ignored"]
fn agent_evals_live_provider_smoke() {
    use datrina_lib::modules::ai::AIEngine;

    let base_url = std::env::var("DATRINA_LIVE_BASE_URL")
        .expect("DATRINA_LIVE_BASE_URL is required for the live lane");
    let api_key = std::env::var("DATRINA_LIVE_API_KEY").ok();
    let model = std::env::var("DATRINA_LIVE_MODEL")
        .expect("DATRINA_LIVE_MODEL is required for the live lane");
    let kind = match std::env::var("DATRINA_LIVE_KIND")
        .unwrap_or_else(|_| "openrouter".to_string())
        .as_str()
    {
        "openrouter" => ProviderKind::Openrouter,
        "ollama" => ProviderKind::Ollama,
        "custom" => ProviderKind::Custom,
        other => panic!(
            "DATRINA_LIVE_KIND={} not recognised — expected openrouter | ollama | custom",
            other
        ),
    };
    if matches!(kind, ProviderKind::Openrouter) && api_key.as_deref().unwrap_or("").is_empty() {
        panic!("DATRINA_LIVE_API_KEY is required when DATRINA_LIVE_KIND=openrouter");
    }
    let provider = LLMProvider {
        id: "live-eval".into(),
        name: "Live Eval".into(),
        kind,
        base_url,
        api_key,
        default_model: model.clone(),
        models: vec![model],
        is_enabled: true,
        is_unsupported: false,
    };
    let prompt = std::env::var("DATRINA_LIVE_PROMPT")
        .unwrap_or_else(|_| "Reply with the literal word OK and nothing else.".to_string());
    let user = ChatMessage {
        id: "live-eval-user".into(),
        role: MessageRole::User,
        content: prompt,
        parts: Vec::new(),
        mode: ChatMode::Context,
        tool_calls: None,
        tool_results: None,
        metadata: None,
        timestamp: 0,
    };
    let engine = AIEngine::default();
    let response = futures_executor::block_on(engine.complete(
        &provider,
        &[user],
        &[],
        StructuredOutputCapability::PlainText,
    ))
    .expect("live provider call must succeed when expensive_evals is opted into");
    assert!(
        !response.content.trim().is_empty(),
        "live provider returned empty content"
    );
    eprintln!(
        "live lane: provider={:?} model={} latency_ms={} strict_mode={:?} content_chars={}",
        provider.kind,
        provider.default_model,
        response.latency_ms,
        response.strict_mode,
        response.content.chars().count()
    );
}

// ─── Self-tests on the runner harness itself ────────────────────────────────

#[test]
fn canonical_json_string_is_key_order_invariant() {
    let a: JsonValue = serde_json::from_str(r#"{"a":1,"b":[true,null,"x"]}"#).unwrap();
    let b: JsonValue = serde_json::from_str(r#"{"b":[true,null,"x"],"a":1}"#).unwrap();
    assert_eq!(canonical_json_string(&a), canonical_json_string(&b));
}

#[test]
fn fixtures_dir_resolves_relative_to_manifest() {
    let dir = fixtures_dir();
    assert!(dir.is_dir(), "fixtures dir missing: {}", dir.display());
}

// Hush unused-import warnings under cfg combos where only some helpers
// are reached. (TokenUsage / BTreeMap / Path are pulled in for symmetry
// with the YAML loader.)
#[allow(dead_code)]
fn _unused_silencer() {
    let _: Option<TokenUsage> = None;
    let _: BTreeMap<String, PlanStep> = BTreeMap::new();
    let _: Option<&Path> = None;
    let _: Option<ModelPricing> = None;
}

// ─── Fault-injection self-test ──────────────────────────────────────────────
//
// The W24 doc says: "Mutate the system prompt to remove the anti-hardcode
// rule; run replay mode; the no_hardcoded_literals assertion fails as
// expected." We can't mutate the prompt from a test, but we *can* prove
// the assertion catches what it's meant to catch by feeding it a synthetic
// proposal with a hardcoded literal and asserting the assertion returns
// an error string. This protects against the assertion silently rotting
// into a no-op after a future validator refactor.

#[test]
fn no_hardcoded_literals_assertion_actually_catches_literals() {
    let proposal: BuildProposal = serde_json::from_value(serde_json::json!({
        "id": "fault-1",
        "title": "Hardcoded stat",
        "widgets": [{
            "widget_type": "stat",
            "title": "Bare literal",
            "data": 0,
            "config": { "value": 99 }
        }],
        "shared_datasources": [],
        "remove_widget_ids": []
    }))
    .expect("synthetic proposal parses");
    let scenario = Scenario {
        id: "fault-1".into(),
        description: String::new(),
        trace: ScenarioTrace {
            proposal,
            tool_calls: Vec::new(),
            plan: None,
            usage: None,
            turns: Vec::new(),
        },
        assertions: Vec::new(),
        mentioned_sources: Vec::new(),
    };
    let err = assert_no_hardcoded_literals(&scenario, &[]);
    assert!(
        err.as_deref().unwrap_or("").contains("Bare literal"),
        "expected the hardcoded-literal assertion to trip on the synthetic offender, got {:?}",
        err
    );
}

#[test]
fn validator_fails_with_assertion_matches_by_variant_tag() {
    let proposal: BuildProposal = serde_json::from_value(serde_json::json!({
        "id": "fault-2",
        "title": "Text dump",
        "widgets": [{
            "widget_type": "text",
            "title": "JSON dump",
            "data": "",
            "datasource_plan": { "kind": "provider_prompt", "prompt": "x" },
            "config": { "content": "{\"k\":1}" }
        }],
        "shared_datasources": [],
        "remove_widget_ids": []
    }))
    .expect("synthetic proposal parses");
    let scenario = Scenario {
        id: "fault-2".into(),
        description: String::new(),
        trace: ScenarioTrace {
            proposal,
            tool_calls: Vec::new(),
            plan: None,
            usage: None,
            turns: Vec::new(),
        },
        assertions: Vec::new(),
        mentioned_sources: Vec::new(),
    };
    assert!(
        assert_validator_fails_with(&scenario, &[], "text_widget_contains_raw_json").is_none(),
        "expected text_widget_contains_raw_json variant to be found"
    );
    let miss = assert_validator_fails_with(&scenario, &[], "duplicate_shared_key");
    assert!(
        miss.is_some(),
        "expected a wrong variant to be reported as missing"
    );
}

// ─── W47 language policy coverage ───────────────────────────────────────────

/// W47: the docs/W47 spec calls out a minimum set of BCP-47 tags the
/// curated catalog must ship. The picker UI dropdowns are populated
/// from this same catalog, so a drift between docs and Rust would
/// silently shrink the surface users see.
#[test]
fn assistant_language_catalog_covers_w47_minimum_set() {
    use datrina_lib::models::language::language_catalog;
    let tags: std::collections::BTreeSet<String> =
        language_catalog().into_iter().map(|o| o.tag).collect();
    for required in [
        "en", "ru", "zh-Hans", "zh-Hant", "ja", "ko", "es", "fr", "de", "pt", "it", "nl", "pl",
        "uk", "tr", "ar", "he", "hi", "bn", "ur", "id", "vi", "th", "ms", "cs", "el", "sv", "no",
        "da", "fi",
    ] {
        assert!(
            tags.contains(required),
            "W47 catalog missing required BCP-47 tag '{required}' — see docs/W47_LLM_CONVERSATION_LANGUAGE_SETTINGS.md",
        );
    }
}

/// W47: the system directive must carry the prompt-name verbatim and
/// reaffirm that schema/tool tokens are never translated. The grounding
/// directive is what every provider (GPT/Claude/Kimi) actually sees, so
/// regressions here would let an OK-looking policy ship with a quietly
/// neutered prompt.
#[test]
fn assistant_language_directive_pins_prompt_name_and_machine_tokens() {
    use datrina_lib::models::language::{
        find_language, AssistantLanguageSource, EffectiveAssistantLanguage,
    };
    let resolved = EffectiveAssistantLanguage {
        source: AssistantLanguageSource::DashboardOverride,
        option: find_language("ru"),
    };
    let directive = resolved
        .system_directive()
        .expect("explicit policy yields a directive");
    assert!(
        directive.contains("Respond in Russian"),
        "directive must use the catalog prompt_name verbatim: {directive}",
    );
    for token in [
        "JSON keys",
        "tool names",
        "widget ids",
        "datasource ids",
        "validation issue codes",
    ] {
        assert!(
            directive.contains(token),
            "directive must reaffirm '{token}' stay untranslated: {directive}",
        );
    }
}

/// W47: `Auto` must never emit a directive — that is what tells the
/// chat / pipeline layer to leave the system prompt alone and follow
/// the user's prompt language. A regression here would silently force
/// every chat into the most recently set explicit language.
#[test]
fn assistant_language_auto_policy_emits_no_directive() {
    use datrina_lib::models::language::EffectiveAssistantLanguage;
    assert!(EffectiveAssistantLanguage::auto()
        .system_directive()
        .is_none());
}

// ─── W51: context compression evals ─────────────────────────────────────────
//
// The bulky-output fixtures below drive
// [`crate::modules::context_compressor`] against representative Datrina
// shapes and assert two things at once:
//
//   1. reduction. The provider-visible payload must be at least the
//      RTK-class target for each fixture (median ≥60%, p90 ≥90%). Without
//      this the compressor is just cosmetic truncation.
//
//   2. fact retention. Status codes, row counts, first-empty step
//      indices, and error messages must survive the compaction pass.
//      Losing them defeats the point — an agent that can't see "row 42
//      failed" can't fix it.
//
// Together these gate the same regressions the spec calls out: large
// HTTP JSON payloads, MCP envelopes with nested JSON text, log/test
// output with one failure, large table datasources, and multi-step
// pipeline traces with one emptying step.

#[test]
fn compression_eval_large_http_payload_meets_reduction_target() {
    use datrina_lib::modules::context_compressor::{compress, CompressionProfile};
    let body: serde_json::Value = serde_json::Value::Array(
        (0..400)
            .map(|i| {
                serde_json::json!({
                    "ts": 1_700_000_000 + i,
                    "city": format!("City-{i}"),
                    "temperature_c": 12.5 + (i as f64) * 0.01,
                    "humidity_pct": 40 + (i % 20),
                    "wind_kph": 6.0 + (i as f64).sin(),
                })
            })
            .collect(),
    );
    let raw = serde_json::json!({
        "method": "GET",
        "url": "https://api.example.com/weather",
        "status": 200,
        "duration_ms": 87,
        "headers": {
            "content-type": "application/json",
            "Authorization": "Bearer sk-supersecret-w51-test-token",
        },
        "body": body,
    });
    let artifact = compress(CompressionProfile::HttpResponse, &raw);

    assert!(
        artifact.reduction_ratio() >= 0.85,
        "W51: HTTP payload reduction {} below 0.85",
        artifact.reduction_ratio()
    );
    // Status survives.
    assert_eq!(
        artifact
            .preserved_facts
            .get("http_status")
            .and_then(serde_json::Value::as_u64),
        Some(200)
    );
    // No bearer token in the provider payload or local preview.
    let provider_payload = artifact.provider_payload();
    let encoded = serde_json::to_string(&provider_payload).unwrap();
    assert!(!encoded.contains("sk-supersecret-w51-test-token"));
    assert!(encoded.contains("[redacted]"));
}

#[test]
fn compression_eval_mcp_envelope_meets_reduction_target() {
    use datrina_lib::modules::context_compressor::{compress, CompressionProfile};
    let inner = serde_json::json!({
        "rows": (0..600)
            .map(|i| serde_json::json!({"i": i, "v": i * 2}))
            .collect::<Vec<_>>(),
        "status": "ok",
        "page_size": 600,
    });
    let raw = serde_json::json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string(&inner).unwrap(),
        }]
    });
    let artifact = compress(CompressionProfile::McpToolResult, &raw);
    assert!(
        artifact.reduction_ratio() >= 0.9,
        "W51: MCP reduction {} below 0.9",
        artifact.reduction_ratio()
    );
    // The model still sees that the envelope unwrapped to an object.
    let envelope_fact = artifact
        .preserved_facts
        .get("envelope")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(envelope_fact.contains("mcp_content"));
}

#[test]
fn compression_eval_pipeline_trace_preserves_first_empty_step() {
    use datrina_lib::modules::context_compressor::{compress, CompressionProfile};
    let mut steps: Vec<serde_json::Value> = Vec::new();
    for i in 0..3 {
        steps.push(serde_json::json!({
            "kind": "fetch",
            "status": "ok",
            "duration_ms": 50 + i,
            "output": (0..200).map(|j| serde_json::json!({"i": j})).collect::<Vec<_>>(),
        }));
    }
    // The emptying step — every downstream consumer should land on this
    // as the diagnostic seed.
    steps.push(serde_json::json!({
        "kind": "filter",
        "status": "ok",
        "duration_ms": 12,
        "output": [],
    }));
    for i in 0..3 {
        steps.push(serde_json::json!({
            "kind": "format",
            "status": "ok",
            "duration_ms": 3 + i,
            "output": [],
        }));
    }
    let raw = serde_json::Value::Array(steps);
    let artifact = compress(CompressionProfile::PipelineTrace, &raw);
    assert_eq!(
        artifact
            .preserved_facts
            .get("first_empty_step_idx")
            .and_then(serde_json::Value::as_u64),
        Some(3)
    );
    assert_eq!(
        artifact
            .preserved_facts
            .get("step_count")
            .and_then(serde_json::Value::as_u64),
        Some(7)
    );
    assert!(artifact.reduction_ratio() >= 0.6);
}

#[test]
fn compression_eval_error_profile_keeps_error_verbatim() {
    use datrina_lib::modules::context_compressor::{compress, CompressionProfile};
    let raw = serde_json::json!({
        "status": "error",
        "error": "validation_failed: WidgetMissingRequiredColumn{column=\"timestamp\"}",
        "context": "x".repeat(20_000),
    });
    let artifact = compress(CompressionProfile::ErrorOrFailure, &raw);
    let err = artifact
        .preserved_facts
        .get("error")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    assert!(err.contains("WidgetMissingRequiredColumn"));
    // Still cuts the context bloat so the provider sees the error
    // without the 20 KB chaff.
    assert!(artifact.reduction_ratio() >= 0.6);
}

#[test]
fn compression_eval_median_and_p90_reductions_meet_rtk_class_targets() {
    use datrina_lib::modules::context_compressor::{compress, CompressionProfile};

    // Five representative provider-visible payloads spanning the
    // profiles W51 cares about. Mirrors the spec's fixture list.
    let payloads: Vec<(CompressionProfile, serde_json::Value)> = vec![
        (
            CompressionProfile::HttpResponse,
            serde_json::json!({
                "status": 200,
                "body": (0..500).map(|i| serde_json::json!({"i": i, "v": "x".repeat(40)})).collect::<Vec<_>>(),
            }),
        ),
        (
            CompressionProfile::McpToolResult,
            serde_json::json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&(0..500).map(|i| serde_json::json!({"i": i})).collect::<Vec<_>>()).unwrap(),
                }]
            }),
        ),
        (
            CompressionProfile::DatasourceSample,
            serde_json::Value::Array(
                (0..1_000).map(|i| serde_json::json!({"row": i, "v": i * 3})).collect(),
            ),
        ),
        (
            CompressionProfile::PipelineTrace,
            serde_json::Value::Array(
                (0..30).map(|i| serde_json::json!({
                    "kind": "fetch",
                    "status": "ok",
                    "duration_ms": 10 + i,
                    "output": (0..200).map(|j| serde_json::json!({"i": j})).collect::<Vec<_>>(),
                })).collect(),
            ),
        ),
        (
            CompressionProfile::ChatToolResult,
            serde_json::json!({
                "rows": (0..400).map(|i| serde_json::json!({"i": i, "label": format!("entry-{i}")})).collect::<Vec<_>>(),
                "count": 400,
            }),
        ),
    ];

    let mut ratios: Vec<f32> = payloads
        .into_iter()
        .map(|(profile, raw)| compress(profile, &raw).reduction_ratio())
        .collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    // Median = middle element of 5 = index 2.
    let median = ratios[2];
    let p90 = ratios[(ratios.len() as f32 * 0.9) as usize - 1];

    assert!(
        median >= 0.6,
        "W51: median compression ratio {} below 0.60 target",
        median
    );
    assert!(
        p90 >= 0.9,
        "W51: p90 compression ratio {} below 0.90 target",
        p90
    );
}

#[test]
fn compression_eval_log_with_failure_keeps_failure_visible() {
    use datrina_lib::modules::context_compressor::{
        compress_text_log, CompressionLimits, CompressionProfile,
    };
    let mut log = String::new();
    for i in 0..400 {
        log.push_str(&format!("INFO pass {i}: ok\n"));
    }
    log.push_str("ERROR: assertion failed at row 42 — expected 7, got 0\n");
    for i in 0..400 {
        log.push_str(&format!("INFO pass-tail {i}: ok\n"));
    }
    let limits = CompressionLimits::for_profile(CompressionProfile::ChatToolResult);
    let (summary, markers) = compress_text_log(&log, &limits);
    let encoded = serde_json::to_string(&summary).unwrap();
    assert!(encoded.contains("assertion failed at row 42"));
    assert!(!markers.is_empty());
    let raw_chars = log.chars().count();
    let compact_chars = encoded.chars().count();
    let ratio = 1.0 - compact_chars as f32 / raw_chars as f32;
    assert!(
        ratio >= 0.9,
        "W51: log compression ratio {} below 0.9",
        ratio
    );
}

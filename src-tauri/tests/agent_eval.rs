//! W24 — agent eval suite (replay mode).
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
//!
//! Scenarios are committed snapshots: they encode the *shape* of a real
//! agent run, not a live re-execution. A future W24 v2 iteration adds an
//! `AIProvider` trait + `MockProvider` to drive the full
//! `send_message_stream_inner` loop from captured chunks; that refactor
//! is intentionally deferred (chat.rs is 4.6k lines and the assertion
//! surfaces below already catch the regressions the suite needs to catch
//! today: prompt drift breaking the validator, anti-hardcode rule decay,
//! pipeline schema rot, cost spikes, and loop-detection drift).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value as JsonValue;

use datrina_lib::commands::validation::validate_build_proposal;
use datrina_lib::models::chat::{
    ChatMessage, ChatMessagePart, ChatMode, MessageRole, PlanStep, PlanStepKind, TokenUsage,
    ToolCall, ToolResult,
};
use datrina_lib::models::dashboard::{BuildProposal, BuildWidgetType};
use datrina_lib::models::pricing::{pricing_for, ModelPricing, UsageReport};
use datrina_lib::models::provider::ProviderKind;
use datrina_lib::models::validation::ValidationIssue;

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
    let issues = validate_build_proposal(&scenario.trace.proposal, None, transcript);
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
    let issues = validate_build_proposal(&scenario.trace.proposal, None, transcript);
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
    }
}

fn assert_no_hardcoded_literals(scenario: &Scenario, transcript: &[ChatMessage]) -> Option<String> {
    let issues = validate_build_proposal(&scenario.trace.proposal, None, transcript);
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
        ProviderKind::LocalMock => "local_mock",
    }
}

// ─── Live mode placeholder ──────────────────────────────────────────────────
//
// W24 v2: when `expensive_evals` is on, this test would swap the replay
// trace for one captured live from OpenRouter. The MockProvider trait
// extraction is the prerequisite — see W24 doc "v2 deferrals" for the
// roadmap. We keep the feature wired so the docs + Cargo manifest stay
// honest about what's available.

#[cfg(feature = "expensive_evals")]
#[test]
#[ignore = "W24 v2 — live mode not yet implemented; AIProvider trait extraction pending"]
fn agent_evals_live_mode_placeholder() {
    panic!(
        "live-mode evals require the AIProvider trait extraction documented \
         in docs/W24_AGENT_EVAL_SUITE.md (v2 deferrals)."
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
        },
        assertions: Vec::new(),
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
        },
        assertions: Vec::new(),
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

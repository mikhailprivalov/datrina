//! W34: option-list resolution for query-backed dashboard parameters.
//!
//! Static-list and constant parameters resolve themselves from the
//! declaration alone (handled by [`super::parameter_engine`] / the
//! `list_dashboard_parameters` command). Query-backed kinds — MCP tool,
//! HTTP request, saved datasource — need a real runtime call. This module
//! is the single place that knows how to:
//!
//! - substitute `$param` tokens inside MCP arguments / HTTP url+body+headers
//!   before issuing the call, so cascading "env → service" selectors work,
//! - build a one-off workflow that runs through [`WorkflowEngine`] (same
//!   path widget refreshes use, so MCP/HTTP behavior is identical),
//! - apply the parameter-specific tail pipeline,
//! - normalize the final value into [`ParameterOption`]s with a forgiving
//!   shape policy (objects with `label`/`value`, plain scalars, or string
//!   arrays all "just work").
//!
//! Failures are returned as `Err(message)` so the caller can surface them
//! per-parameter (`options_error`) without poisoning the whole dashboard
//! parameter list.

use anyhow::{anyhow, Result as AnyResult};
use serde_json::Value;
use tauri::State;

use crate::commands::dashboard::{
    active_provider, datasource_plan_workflow, reconnect_enabled_mcp_servers,
};
use crate::models::dashboard::{
    BuildDatasourcePlan, BuildDatasourcePlanKind, BuildWidgetProposal, BuildWidgetType,
    DashboardParameter, DashboardParameterKind, ParameterOption, ParameterValue,
};
use crate::models::pipeline::PipelineStep;
use crate::models::workflow::RunStatus;
use crate::modules::parameter_engine::{ResolvedParameters, SubstituteOptions};
use crate::modules::workflow_engine::WorkflowEngine;
use crate::AppState;

/// Resolve the option list for one parameter. Returns `Ok(options)` for
/// every kind (including the trivial ones); query-backed kinds that fail
/// return `Err`, which the caller surfaces as `options_error` so the UI
/// can show inline error and keep the previously selected value.
pub async fn resolve_options_for_parameter(
    state: &State<'_, AppState>,
    param: &DashboardParameter,
    upstream: &ResolvedParameters,
) -> AnyResult<Vec<ParameterOption>> {
    match &param.kind {
        DashboardParameterKind::StaticList { options } => Ok(options.clone()),
        DashboardParameterKind::TextInput { .. }
        | DashboardParameterKind::TimeRange { .. }
        | DashboardParameterKind::Interval { .. }
        | DashboardParameterKind::Constant { .. } => Ok(Vec::new()),
        DashboardParameterKind::McpQuery {
            server_id,
            tool_name,
            arguments,
            pipeline,
        } => {
            let substituted_args = substitute_value(arguments.clone(), upstream);
            let plan = BuildDatasourcePlan {
                kind: if server_id == "builtin" {
                    BuildDatasourcePlanKind::BuiltinTool
                } else {
                    BuildDatasourcePlanKind::McpTool
                },
                tool_name: Some(tool_name.clone()),
                server_id: Some(server_id.clone()),
                arguments: substituted_args,
                prompt: None,
                output_path: None,
                refresh_cron: None,
                pipeline: pipeline.clone(),
                source_key: None,
                inputs: None,
            };
            let final_value = run_query_plan(state, &param.name, plan).await?;
            normalize_options(final_value)
        }
        DashboardParameterKind::HttpQuery {
            method,
            url,
            headers,
            body,
            pipeline,
        } => {
            let substituted_url = upstream.substitute_string(url, SubstituteOptions::default());
            let mut args = serde_json::Map::new();
            args.insert("method".into(), Value::String(method.clone()));
            args.insert("url".into(), Value::String(substituted_url));
            if let Some(headers) = headers.clone() {
                if let Some(value) = substitute_value(Some(headers), upstream) {
                    args.insert("headers".into(), value);
                }
            }
            if let Some(body) = body.clone() {
                if let Some(value) = substitute_value(Some(body), upstream) {
                    args.insert("body".into(), value);
                }
            }
            let plan = BuildDatasourcePlan {
                kind: BuildDatasourcePlanKind::BuiltinTool,
                tool_name: Some("http_request".into()),
                server_id: Some("builtin".into()),
                arguments: Some(Value::Object(args)),
                prompt: None,
                output_path: None,
                refresh_cron: None,
                pipeline: pipeline.clone(),
                source_key: None,
                inputs: None,
            };
            let final_value = run_query_plan(state, &param.name, plan).await?;
            normalize_options(final_value)
        }
        DashboardParameterKind::DatasourceQuery {
            datasource_id,
            pipeline,
        } => {
            let def = state
                .storage
                .get_datasource_definition(datasource_id)
                .await?
                .ok_or_else(|| {
                    anyhow!(
                        "Datasource '{}' for parameter '{}' not found",
                        datasource_id,
                        param.name
                    )
                })?;
            // Build the saved datasource's own plan (mcp_tool / builtin_tool
            // / provider_prompt) and append the parameter-specific tail
            // pipeline. We do not call `run_datasource_definition` directly
            // because that path persists workflow runs and updates health
            // counters — option resolution is read-only and should not
            // dirty the catalog with every dashboard render.
            let combined_pipeline: Vec<PipelineStep> = def
                .pipeline
                .iter()
                .cloned()
                .chain(pipeline.iter().cloned())
                .collect();
            let plan = BuildDatasourcePlan {
                kind: def.kind.clone(),
                tool_name: def.tool_name.clone(),
                server_id: def.server_id.clone(),
                arguments: def.arguments.clone(),
                prompt: def.prompt.clone(),
                output_path: None,
                refresh_cron: None,
                pipeline: combined_pipeline,
                source_key: None,
                inputs: None,
            };
            let final_value = run_query_plan(state, &param.name, plan).await?;
            normalize_options(final_value)
        }
    }
}

fn substitute_value(value: Option<Value>, resolved: &ResolvedParameters) -> Option<Value> {
    value.map(|v| resolved.substitute_value(&v, SubstituteOptions::default()))
}

/// Build a one-off workflow from `plan`, run it through the engine, and
/// return the final output value. Mirrors the shape of `run_definition_once`
/// but never persists a workflow run.
async fn run_query_plan(
    state: &State<'_, AppState>,
    param_name: &str,
    plan: BuildDatasourcePlan,
) -> AnyResult<Value> {
    reconnect_enabled_mcp_servers(state).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let synthetic = BuildWidgetProposal {
        widget_type: BuildWidgetType::Text,
        title: format!("param:{}", param_name),
        data: Value::Null,
        datasource_plan: Some(plan.clone()),
        config: None,
        x: None,
        y: None,
        w: None,
        h: None,
        replace_widget_id: None,
        size_preset: None,
        layout_pattern: None,
    };
    let workflow_id = format!("__param_opts__{}__{}", param_name, now);
    let workflow = datasource_plan_workflow(
        workflow_id,
        format!("Parameter options: {}", param_name),
        &synthetic,
        &plan,
        now,
    )?;
    // W47: parameter-option queries inherit the dashboard's language
    // policy when the caller passes one; today this resolver runs
    // without dashboard scope, so it falls back to the app default.
    let language_directive =
        crate::commands::language::resolve_effective_language(state.storage.as_ref(), None, None)
            .await
            .ok()
            .and_then(|resolved| resolved.system_directive());
    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        active_provider(state).await?,
    )
    .with_language(language_directive);
    let execution = engine.execute(&workflow, None).await?;
    let run = execution.run;
    if !matches!(run.status, RunStatus::Success) {
        return Err(anyhow!(run
            .error
            .unwrap_or_else(|| "parameter option workflow failed".to_string())));
    }
    let node_results = run
        .node_results
        .as_ref()
        .ok_or_else(|| anyhow!("parameter option workflow returned no node results"))?;
    let final_value = node_results
        .get("output")
        .and_then(|out| out.get("data"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(final_value)
}

/// Convert a pipeline output into a list of `{label, value}`. Forgiving:
/// - `[{label, value}, ...]` is used as-is,
/// - `[{name, id}, ...]` and `[{title, value}, ...]` map common synonyms,
/// - `["a", "b"]` or `[1, 2]` becomes `[{label: "a", value: "a"}, ...]`,
/// - `{ "a": "...", "b": "..." }` becomes `[{label: "a", value: "a"}, ...]`,
/// - anything else returns an explicit error so the UI can show why no
///   options appeared instead of rendering an empty (and silent) dropdown.
pub fn normalize_options(value: Value) -> AnyResult<Vec<ParameterOption>> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .into_iter()
            .map(option_from_item)
            .collect::<AnyResult<Vec<_>>>(),
        Value::Object(map) => Ok(map
            .into_iter()
            .map(|(k, _)| ParameterOption {
                label: k.clone(),
                value: ParameterValue::String(k),
            })
            .collect()),
        Value::String(_) | Value::Number(_) | Value::Bool(_) => Ok(vec![option_from_item(value)?]),
    }
}

fn option_from_item(item: Value) -> AnyResult<ParameterOption> {
    if let Value::Object(map) = &item {
        let label = pick_string_field(map, &["label", "name", "title", "text"]);
        let value = pick_value_field(map, &["value", "id", "key"]);
        if let (Some(label), Some(value)) = (label, value) {
            return Ok(ParameterOption {
                label,
                value: parameter_value_from_json(value)?,
            });
        }
        // Object without the expected shape — fail loud rather than render
        // mystery rows. Operators usually want to fix the tail pipeline.
        return Err(anyhow!(
            "option item is an object but lacks a usable label/value pair (looked for label/name/title and value/id/key)"
        ));
    }
    let scalar = parameter_value_from_json(item.clone())?;
    let label = scalar_to_label(&item);
    Ok(ParameterOption {
        label,
        value: scalar,
    })
}

fn pick_string_field(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(Value::String(s)) = map.get(*key) {
            return Some(s.clone());
        }
    }
    // Fall back to the JSON encoding of numeric / bool labels so callers
    // pointing at `{ id: 7 }` still see a usable row.
    for key in keys {
        if let Some(value) = map.get(*key) {
            if let Some(rendered) = render_scalar(value) {
                return Some(rendered);
            }
        }
    }
    None
}

fn pick_value_field(map: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<Value> {
    for key in keys {
        if let Some(value) = map.get(*key) {
            return Some(value.clone());
        }
    }
    None
}

fn render_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn scalar_to_label(value: &Value) -> String {
    render_scalar(value).unwrap_or_else(|| value.to_string())
}

fn parameter_value_from_json(value: Value) -> AnyResult<ParameterValue> {
    match value {
        Value::String(s) => Ok(ParameterValue::String(s)),
        Value::Bool(b) => Ok(ParameterValue::Bool(b)),
        Value::Number(n) => n
            .as_f64()
            .map(ParameterValue::Number)
            .ok_or_else(|| anyhow!("option value number is not representable as f64")),
        Value::Array(items) => items
            .into_iter()
            .map(parameter_value_from_json)
            .collect::<AnyResult<Vec<_>>>()
            .map(ParameterValue::Array),
        Value::Null => Err(anyhow!("option value cannot be null")),
        Value::Object(_) => Err(anyhow!(
            "option value cannot be an object — pick a scalar field with a tail pipeline step"
        )),
    }
}

/// Re-run option resolution for an entire parameter list in topological
/// order, propagating each resolved value into the upstream substitution
/// context. Used by `list_dashboard_parameters` so cascading selectors
/// just work without an extra round-trip per dependency edge. Returns
/// `(options_by_name, error_by_name, resolved_values)` so the caller can
/// build a typed envelope (and reuse `resolved_values` for things like
/// affected-widget computation).
pub async fn resolve_all_parameter_options(
    state: &State<'_, AppState>,
    params: &[DashboardParameter],
    selected: &std::collections::BTreeMap<String, ParameterValue>,
) -> (
    std::collections::BTreeMap<String, Vec<ParameterOption>>,
    std::collections::BTreeMap<String, String>,
    ResolvedParameters,
) {
    use std::collections::BTreeMap;
    let mut options_map: BTreeMap<String, Vec<ParameterOption>> = BTreeMap::new();
    let mut errors_map: BTreeMap<String, String> = BTreeMap::new();
    let mut current_values: BTreeMap<String, ParameterValue> = BTreeMap::new();

    // Iterate in dependency order so query parameters can substitute
    // already-resolved upstream values. Fall back to declaration order
    // if cycle detection trips — the caller already surfaces the cycle.
    let order = match topological_names(params) {
        Some(names) => names,
        None => params.iter().map(|p| p.name.clone()).collect(),
    };
    for name in order {
        let Some(param) = params.iter().find(|p| p.name == name) else {
            continue;
        };
        let upstream = ResolvedParameters::from_map(current_values.clone());
        let chosen = match selected.get(&param.name) {
            Some(v) => Some(v.clone()),
            None => param.default.clone(),
        };
        match resolve_options_for_parameter(state, param, &upstream).await {
            Ok(options) => {
                options_map.insert(param.name.clone(), options.clone());
                // Pick a current value: explicit selection > default >
                // first option (so dependent queries see *something*).
                let effective = chosen
                    .clone()
                    .or_else(|| options.first().map(|o| o.value.clone()));
                if let Some(value) = effective {
                    current_values.insert(param.name.clone(), value);
                } else if let Some(default) = param.default.clone() {
                    current_values.insert(param.name.clone(), default);
                }
            }
            Err(error) => {
                errors_map.insert(param.name.clone(), error.to_string());
                // Even on error we still propagate the user's previous
                // selection so downstream queries don't drift to "" when
                // an upstream backend is temporarily down.
                if let Some(value) = chosen {
                    current_values.insert(param.name.clone(), value);
                }
            }
        }
    }
    let resolved = ResolvedParameters::from_map(current_values);
    (options_map, errors_map, resolved)
}

/// Topological order over `depends_on`. Returns `None` on cycles so the
/// caller can decide whether to fall back to declaration order. Mirrors
/// the algorithm in [`super::parameter_engine`] without exposing the
/// graph type itself.
fn topological_names(params: &[DashboardParameter]) -> Option<Vec<String>> {
    use std::collections::{HashMap, HashSet};
    let name_set: HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for param in params {
        in_degree.entry(param.name.clone()).or_insert(0);
        adjacency.entry(param.name.clone()).or_default();
    }
    for param in params {
        for dep in &param.depends_on {
            if !name_set.contains(dep.as_str()) {
                continue;
            }
            adjacency
                .entry(dep.clone())
                .or_default()
                .push(param.name.clone());
            *in_degree.entry(param.name.clone()).or_insert(0) += 1;
        }
    }
    let mut ready: Vec<String> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| name.clone())
        .collect();
    ready.sort();
    let mut order = Vec::with_capacity(params.len());
    while let Some(name) = ready.pop() {
        if let Some(children) = adjacency.get(&name).cloned() {
            order.push(name);
            let mut to_enqueue = Vec::new();
            for child in children {
                if let Some(deg) = in_degree.get_mut(&child) {
                    *deg -= 1;
                    if *deg == 0 {
                        to_enqueue.push(child);
                    }
                }
            }
            to_enqueue.sort();
            ready.extend(to_enqueue);
        }
    }
    if order.len() == params.len() {
        Some(order)
    } else {
        None
    }
}

/// Return the names of every parameter that should be re-resolved after
/// `changed_name` updates. This is just the transitive set of
/// `depends_on` consumers; cycles fall back to "every param" because the
/// caller already surfaces the cycle error.
pub fn downstream_dependents(params: &[DashboardParameter], changed_name: &str) -> Vec<String> {
    use std::collections::{BTreeSet, VecDeque};
    let mut affected: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(changed_name.to_string());
    while let Some(name) = queue.pop_front() {
        for param in params {
            if param.depends_on.iter().any(|d| d == &name) && affected.insert(param.name.clone()) {
                queue.push_back(param.name.clone());
            }
        }
    }
    affected.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_label_value_objects() {
        let value = json!([
            {"label": "Alpha", "value": "alpha"},
            {"label": "Beta", "value": "beta"},
        ]);
        let opts = normalize_options(value).unwrap();
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[0].label, "Alpha");
        assert_eq!(opts[0].value, ParameterValue::String("alpha".into()));
    }

    #[test]
    fn normalize_name_id_synonyms() {
        let value = json!([{"name": "prod", "id": 1}, {"name": "stage", "id": 2}]);
        let opts = normalize_options(value).unwrap();
        assert_eq!(opts[0].label, "prod");
        assert_eq!(opts[0].value, ParameterValue::Number(1.0));
    }

    #[test]
    fn normalize_string_array_doubles_label_and_value() {
        let value = json!(["alpha", "beta"]);
        let opts = normalize_options(value).unwrap();
        assert_eq!(opts[0].label, "alpha");
        assert_eq!(opts[0].value, ParameterValue::String("alpha".into()));
    }

    #[test]
    fn normalize_object_uses_keys() {
        let value = json!({"alpha": 1, "beta": 2});
        let opts = normalize_options(value).unwrap();
        let labels: Vec<_> = opts.iter().map(|o| o.label.clone()).collect();
        assert!(labels.contains(&"alpha".to_string()));
        assert!(labels.contains(&"beta".to_string()));
    }

    #[test]
    fn normalize_object_without_value_field_errors() {
        let value = json!([{"only_label": "x"}]);
        let err = normalize_options(value).unwrap_err();
        assert!(err.to_string().contains("label/value"));
    }

    fn declare(
        name: &str,
        kind: DashboardParameterKind,
        depends_on: Vec<&str>,
    ) -> DashboardParameter {
        DashboardParameter {
            id: name.to_string(),
            name: name.to_string(),
            label: name.to_string(),
            kind,
            multi: false,
            include_all: false,
            default: None,
            depends_on: depends_on.into_iter().map(String::from).collect(),
            description: None,
        }
    }

    #[test]
    fn downstream_dependents_walks_transitively() {
        let params = vec![
            declare(
                "env",
                DashboardParameterKind::StaticList {
                    options: Vec::new(),
                },
                vec![],
            ),
            declare(
                "service",
                DashboardParameterKind::StaticList {
                    options: Vec::new(),
                },
                vec!["env"],
            ),
            declare(
                "version",
                DashboardParameterKind::StaticList {
                    options: Vec::new(),
                },
                vec!["service"],
            ),
            declare(
                "unrelated",
                DashboardParameterKind::StaticList {
                    options: Vec::new(),
                },
                vec![],
            ),
        ];
        let affected = downstream_dependents(&params, "env");
        assert_eq!(affected, vec!["service".to_string(), "version".to_string()]);
        let none = downstream_dependents(&params, "unrelated");
        assert!(none.is_empty());
    }

    #[test]
    fn topological_names_orders_parents_first() {
        let params = vec![
            declare(
                "child",
                DashboardParameterKind::StaticList {
                    options: Vec::new(),
                },
                vec!["parent"],
            ),
            declare(
                "parent",
                DashboardParameterKind::StaticList {
                    options: Vec::new(),
                },
                vec![],
            ),
        ];
        let order = topological_names(&params).expect("DAG resolves");
        assert_eq!(order, vec!["parent".to_string(), "child".to_string()]);
    }
}

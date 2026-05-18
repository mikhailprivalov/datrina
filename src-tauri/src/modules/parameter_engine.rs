//! W25 Dashboard Parameters substitution engine.
//!
//! Dashboards declare a typed list of `parameters`. Widget configs reference
//! them as `$name` or `${name}` (Grafana style) inside MCP arguments, HTTP
//! query strings, and pipeline step configs. This module resolves the
//! current value for every parameter (applying user selections, defaults,
//! dependency cascades) and substitutes those values into arbitrary JSON
//! before the workflow engine sees it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::models::dashboard::{
    DashboardParameter, DashboardParameterKind, ParameterValue, TimeRangeValue,
};

/// How a multi-value parameter (Array) renders when substituted into a
/// scalar string context. Most call sites want a comma-joined list; SQL
/// IN-clauses want a JSON array; raw substitution keeps the first value.
#[derive(Debug, Clone, Copy, Default)]
pub enum MultiRender {
    #[default]
    CommaJoin,
    JsonArray,
    First,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SubstituteOptions {
    pub multi_render: MultiRender,
}

/// Resolved parameter map: every declared parameter's current value (user
/// selection, default, or first-option fallback).
#[derive(Debug, Default, Clone)]
pub struct ResolvedParameters {
    values: BTreeMap<String, ParameterValue>,
}

impl ResolvedParameters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_map(values: BTreeMap<String, ParameterValue>) -> Self {
        Self { values }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&ParameterValue> {
        self.values.get(name)
    }

    pub fn as_map(&self) -> &BTreeMap<String, ParameterValue> {
        &self.values
    }

    pub fn insert(&mut self, name: impl Into<String>, value: ParameterValue) {
        self.values.insert(name.into(), value);
    }

    /// Build a resolved view from parameter declarations + user selections.
    /// Cycles in `depends_on` produce a `ParameterCycle` error. Query-backed
    /// parameter kinds are not resolved against their backend here — the
    /// stored selection (or default / first option) is used. Option list
    /// resolution lives in the dashboard commands.
    pub fn resolve(
        params: &[DashboardParameter],
        selected: &BTreeMap<String, ParameterValue>,
    ) -> Result<Self> {
        let order = topological_order(params)?;
        let mut values: BTreeMap<String, ParameterValue> = BTreeMap::new();

        for name in order {
            let Some(param) = params.iter().find(|p| p.name == name) else {
                continue;
            };
            let chosen = if let Some(value) = selected.get(&param.name) {
                value.clone()
            } else if let Some(default) = param.default.clone() {
                default
            } else {
                default_value_for_kind(&param.kind).unwrap_or(ParameterValue::String(String::new()))
            };
            values.insert(param.name.clone(), chosen);
        }

        Ok(Self { values })
    }

    /// Walk `value` and replace every `$name` / `${name}` token. For tokens
    /// referencing a `TimeRange` parameter, `$name.from`, `$name.to`, and
    /// `$name.duration_ms` are exposed as scalar accessors. Whole-string
    /// tokens preserve the parameter's JSON type (e.g. a Number param
    /// substituted into a numeric field stays a JSON number).
    pub fn substitute_value(&self, value: &Value, options: SubstituteOptions) -> Value {
        if self.values.is_empty() {
            return value.clone();
        }
        match value {
            Value::String(s) => self.substitute_string_value(s, options),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.substitute_value(item, options))
                    .collect(),
            ),
            Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(k.clone(), self.substitute_value(v, options));
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }

    /// Substitute into a raw string (multi-values flattened per `options`).
    pub fn substitute_string(&self, raw: &str, options: SubstituteOptions) -> String {
        match self.substitute_string_value(raw, options) {
            Value::String(s) => s,
            other => other.to_string(),
        }
    }

    fn substitute_string_value(&self, raw: &str, options: SubstituteOptions) -> Value {
        let tokens = scan_tokens(raw);
        if tokens.is_empty() {
            return Value::String(raw.to_string());
        }
        // Whole-string token: preserve original parameter type.
        if tokens.len() == 1 {
            let token = &tokens[0];
            if token.start == 0 && token.end == raw.len() {
                return self
                    .lookup_token(&token.name, options)
                    .unwrap_or(Value::String(String::new()));
            }
        }
        // Mixed string: produce a string with each token rendered to its
        // scalar form.
        let mut out = String::with_capacity(raw.len());
        let mut cursor = 0;
        for token in &tokens {
            out.push_str(&raw[cursor..token.start]);
            let scalar = self
                .lookup_token(&token.name, options)
                .map(value_to_scalar_string)
                .unwrap_or_default();
            out.push_str(&scalar);
            cursor = token.end;
        }
        out.push_str(&raw[cursor..]);
        Value::String(out)
    }

    fn lookup_token(&self, token: &str, options: SubstituteOptions) -> Option<Value> {
        let (name, accessor) = match token.split_once('.') {
            Some((n, a)) => (n, Some(a)),
            None => (token, None),
        };
        let value = self.values.get(name)?;
        Some(parameter_to_value(value, accessor, options))
    }

    /// Collect every `$name` / `${name}` reference (top-level name only,
    /// strips any `.accessor`) found anywhere in `value`. Used to compute
    /// the affected-widget set on a parameter change.
    pub fn referenced_names(value: &Value) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        collect_refs(value, &mut out);
        out
    }
}

fn collect_refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s) => {
            for token in scan_tokens(s) {
                let name = token
                    .name
                    .split('.')
                    .next()
                    .map(|s| s.to_string())
                    .unwrap_or(token.name);
                out.insert(name);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_refs(i, out)),
        Value::Object(map) => map.values().for_each(|v| collect_refs(v, out)),
        _ => {}
    }
}

/// Convert a `ParameterValue` to a JSON value with the requested accessor
/// applied. `accessor` selects sub-fields on TimeRange parameters.
fn parameter_to_value(
    value: &ParameterValue,
    accessor: Option<&str>,
    options: SubstituteOptions,
) -> Value {
    match (value, accessor) {
        (ParameterValue::Range(range), Some(field)) => match field {
            "from" => Value::from(range.from),
            "to" => Value::from(range.to),
            "duration_ms" => Value::from(range.to - range.from),
            _ => Value::Null,
        },
        (ParameterValue::Range(range), None) => serde_json::json!({
            "from": range.from,
            "to": range.to,
        }),
        (ParameterValue::Array(items), _) => match options.multi_render {
            MultiRender::JsonArray => {
                Value::Array(items.iter().map(|item| scalar_to_value(item)).collect())
            }
            MultiRender::First => items.first().map(scalar_to_value).unwrap_or(Value::Null),
            MultiRender::CommaJoin => {
                let joined = items
                    .iter()
                    .map(value_inner_string)
                    .collect::<Vec<_>>()
                    .join(",");
                Value::String(joined)
            }
        },
        (ParameterValue::String(s), _) => Value::String(s.clone()),
        (ParameterValue::Number(n), _) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        (ParameterValue::Bool(b), _) => Value::Bool(*b),
    }
}

fn scalar_to_value(value: &ParameterValue) -> Value {
    match value {
        ParameterValue::String(s) => Value::String(s.clone()),
        ParameterValue::Number(n) => serde_json::Number::from_f64(*n)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        ParameterValue::Bool(b) => Value::Bool(*b),
        ParameterValue::Array(_) | ParameterValue::Range(_) => Value::Null,
    }
}

fn value_inner_string(value: &ParameterValue) -> String {
    match value {
        ParameterValue::String(s) => s.clone(),
        ParameterValue::Number(n) => format!("{}", n),
        ParameterValue::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn value_to_scalar_string(value: Value) -> String {
    match value {
        Value::String(s) => s,
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => items
            .iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(","),
        other => other.to_string(),
    }
}

fn default_value_for_kind(kind: &DashboardParameterKind) -> Option<ParameterValue> {
    match kind {
        DashboardParameterKind::StaticList { options } => {
            options.first().map(|opt| opt.value.clone())
        }
        DashboardParameterKind::TextInput { .. } => Some(ParameterValue::String(String::new())),
        DashboardParameterKind::Constant { value } => Some(value.clone()),
        DashboardParameterKind::Interval { presets } => {
            presets.first().cloned().map(ParameterValue::String)
        }
        DashboardParameterKind::TimeRange { .. } => {
            let now = chrono::Utc::now().timestamp_millis();
            Some(ParameterValue::Range(TimeRangeValue {
                from: now - 60 * 60 * 1000,
                to: now,
            }))
        }
        DashboardParameterKind::McpQuery { .. }
        | DashboardParameterKind::HttpQuery { .. }
        | DashboardParameterKind::DatasourceQuery { .. } => None,
    }
}

#[derive(Debug, Clone)]
struct Token {
    name: String,
    start: usize,
    end: usize,
}

/// Locate every `$identifier` and `${identifier}` reference, where
/// `identifier = [A-Za-z_][A-Za-z0-9_]* ( '.' [A-Za-z_][A-Za-z0-9_]* )?`.
fn scan_tokens(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            i += 1;
            continue;
        }
        // Allow doubled `$$` as an escape: skip the next byte.
        if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
            i += 2;
            continue;
        }
        let start = i;
        let braced = i + 1 < bytes.len() && bytes[i + 1] == b'{';
        let name_start = if braced { i + 2 } else { i + 1 };
        let mut j = name_start;
        let mut saw_dot = false;
        while j < bytes.len() {
            let c = bytes[j];
            let is_first_char = j == name_start || (saw_dot && bytes[j - 1] == b'.');
            let allowed = if is_first_char {
                c.is_ascii_alphabetic() || c == b'_'
            } else {
                c.is_ascii_alphanumeric() || c == b'_' || (c == b'.' && !saw_dot)
            };
            if !allowed {
                break;
            }
            if c == b'.' {
                saw_dot = true;
            }
            j += 1;
        }
        if j == name_start {
            // `$` followed by something that's not an identifier — skip.
            i += 1;
            continue;
        }
        let name = input[name_start..j].to_string();
        let end = if braced {
            if j < bytes.len() && bytes[j] == b'}' {
                j + 1
            } else {
                // Unclosed ${ — skip this token.
                i = j;
                continue;
            }
        } else {
            j
        };
        out.push(Token { name, start, end });
        i = end;
    }
    out
}

/// Walk every `WorkflowNode.config` value and substitute parameters in
/// place. No-op when `resolved` is empty.
pub fn substitute_workflow(
    workflow: &mut crate::models::workflow::Workflow,
    resolved: &ResolvedParameters,
    options: SubstituteOptions,
) {
    if resolved.is_empty() {
        return;
    }
    for node in &mut workflow.nodes {
        if let Some(cfg) = node.config.as_mut() {
            *cfg = resolved.substitute_value(cfg, options);
        }
    }
}

/// Topological order over `depends_on` edges. Fails with a cycle error so
/// callers (the W16 validator + the resolve command) can surface it. The
/// returned order lists parent params before children.
fn topological_order(params: &[DashboardParameter]) -> Result<Vec<String>> {
    let name_set: HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for param in params {
        in_degree.entry(param.name.as_str()).or_insert(0);
        adjacency.entry(param.name.as_str()).or_default();
    }
    for param in params {
        for dep in &param.depends_on {
            if !name_set.contains(dep.as_str()) {
                continue; // dangling deps don't block resolution
            }
            adjacency
                .entry(dep.as_str())
                .or_default()
                .push(param.name.as_str());
            *in_degree.entry(param.name.as_str()).or_insert(0) += 1;
        }
    }
    let mut ready: Vec<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| *name)
        .collect();
    ready.sort();
    let mut order = Vec::with_capacity(params.len());
    while let Some(name) = ready.pop() {
        order.push(name.to_string());
        if let Some(children) = adjacency.get(name) {
            let mut to_enqueue = Vec::new();
            for child in children {
                if let Some(deg) = in_degree.get_mut(child) {
                    *deg -= 1;
                    if *deg == 0 {
                        to_enqueue.push(*child);
                    }
                }
            }
            to_enqueue.sort();
            ready.extend(to_enqueue);
        }
    }
    if order.len() != params.len() {
        let cycle: Vec<String> = params
            .iter()
            .filter(|p| {
                in_degree
                    .get(p.name.as_str())
                    .map(|deg| *deg > 0)
                    .unwrap_or(false)
            })
            .map(|p| p.name.clone())
            .collect();
        return Err(anyhow!(
            "dashboard parameter dependency cycle: {}",
            cycle.join(" → ")
        ));
    }
    Ok(order)
}

/// Detect a cycle in the parameter dependency graph. Returns the names that
/// participate in the cycle (best-effort: every node with non-zero
/// in-degree after Kahn's algorithm). Returns `None` when the graph is a
/// DAG.
pub fn detect_cycle(params: &[DashboardParameter]) -> Option<Vec<String>> {
    match topological_order(params) {
        Ok(_) => None,
        Err(_) => {
            let mut cycle: Vec<String> = Vec::new();
            let name_set: HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
            let mut in_degree: HashMap<&str, usize> = HashMap::new();
            for param in params {
                in_degree.entry(param.name.as_str()).or_insert(0);
            }
            for param in params {
                for dep in &param.depends_on {
                    if !name_set.contains(dep.as_str()) {
                        continue;
                    }
                    *in_degree.entry(param.name.as_str()).or_insert(0) += 1;
                }
            }
            // Iterate Kahn's algorithm and collect the survivors.
            let mut ready: Vec<&str> = in_degree
                .iter()
                .filter(|(_, deg)| **deg == 0)
                .map(|(n, _)| *n)
                .collect();
            let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
            for param in params {
                for dep in &param.depends_on {
                    if !name_set.contains(dep.as_str()) {
                        continue;
                    }
                    adjacency
                        .entry(dep.as_str())
                        .or_default()
                        .push(param.name.as_str());
                }
            }
            while let Some(name) = ready.pop() {
                if let Some(children) = adjacency.get(name) {
                    for child in children {
                        if let Some(deg) = in_degree.get_mut(child) {
                            *deg -= 1;
                            if *deg == 0 {
                                ready.push(*child);
                            }
                        }
                    }
                }
            }
            for (name, deg) in &in_degree {
                if *deg > 0 {
                    cycle.push((*name).to_string());
                }
            }
            cycle.sort();
            if cycle.is_empty() {
                None
            } else {
                Some(cycle)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::dashboard::{
        DashboardParameter, DashboardParameterKind, ParameterOption, ParameterValue,
    };
    use serde_json::json;

    fn p(name: &str, kind: DashboardParameterKind) -> DashboardParameter {
        DashboardParameter {
            id: name.to_string(),
            name: name.to_string(),
            label: name.to_string(),
            kind,
            multi: false,
            include_all: false,
            default: None,
            depends_on: Vec::new(),
            description: None,
        }
    }

    #[test]
    fn whole_string_token_preserves_number_type() {
        let mut resolved = ResolvedParameters::new();
        resolved.insert("count", ParameterValue::Number(42.0));
        let input = json!({ "count": "$count" });
        let out = resolved.substitute_value(&input, SubstituteOptions::default());
        assert!(out.get("count").unwrap().is_number());
        assert_eq!(out["count"].as_f64().unwrap(), 42.0);
    }

    #[test]
    fn whole_string_token_preserves_bool_type() {
        let mut resolved = ResolvedParameters::new();
        resolved.insert("enabled", ParameterValue::Bool(true));
        let input = json!({ "flag": "${enabled}" });
        let out = resolved.substitute_value(&input, SubstituteOptions::default());
        assert_eq!(out["flag"], json!(true));
    }

    #[test]
    fn mixed_string_renders_scalar() {
        let mut resolved = ResolvedParameters::new();
        resolved.insert("env", ParameterValue::String("prod".into()));
        let input = json!("https://api/$env/v1");
        let out = resolved.substitute_value(&input, SubstituteOptions::default());
        assert_eq!(out, json!("https://api/prod/v1"));
    }

    #[test]
    fn multi_render_comma_join_and_json_array() {
        let mut resolved = ResolvedParameters::new();
        resolved.insert(
            "tags",
            ParameterValue::Array(vec![
                ParameterValue::String("a".into()),
                ParameterValue::String("b".into()),
                ParameterValue::String("c".into()),
            ]),
        );
        let comma = resolved.substitute_value(
            &json!("$tags"),
            SubstituteOptions {
                multi_render: MultiRender::CommaJoin,
            },
        );
        assert_eq!(comma, json!("a,b,c"));
        let arr = resolved.substitute_value(
            &json!("$tags"),
            SubstituteOptions {
                multi_render: MultiRender::JsonArray,
            },
        );
        assert_eq!(arr, json!(["a", "b", "c"]));
    }

    #[test]
    fn time_range_accessor_exposes_from_to_duration() {
        let mut resolved = ResolvedParameters::new();
        resolved.insert(
            "range",
            ParameterValue::Range(TimeRangeValue {
                from: 1000,
                to: 5000,
            }),
        );
        let input = json!({ "from": "$range.from", "to": "$range.to", "ms": "$range.duration_ms" });
        let out = resolved.substitute_value(&input, SubstituteOptions::default());
        assert_eq!(out["from"], json!(1000));
        assert_eq!(out["to"], json!(5000));
        assert_eq!(out["ms"], json!(4000));
    }

    #[test]
    fn cycle_detection() {
        let mut a = p(
            "a",
            DashboardParameterKind::StaticList {
                options: Vec::new(),
            },
        );
        a.depends_on = vec!["b".into()];
        let mut b = p(
            "b",
            DashboardParameterKind::StaticList {
                options: Vec::new(),
            },
        );
        b.depends_on = vec!["a".into()];
        let cycle = detect_cycle(&[a, b]).expect("cycle should be detected");
        assert!(cycle.contains(&"a".to_string()));
        assert!(cycle.contains(&"b".to_string()));
    }

    #[test]
    fn cascading_resolve_uses_topological_order() {
        let mut env = p(
            "env",
            DashboardParameterKind::StaticList {
                options: vec![ParameterOption {
                    label: "prod".into(),
                    value: ParameterValue::String("prod".into()),
                }],
            },
        );
        env.default = Some(ParameterValue::String("prod".into()));
        let mut service = p(
            "service",
            DashboardParameterKind::StaticList {
                options: vec![ParameterOption {
                    label: "api".into(),
                    value: ParameterValue::String("api".into()),
                }],
            },
        );
        service.depends_on = vec!["env".into()];
        service.default = Some(ParameterValue::String("api".into()));
        let resolved =
            ResolvedParameters::resolve(&[env, service], &BTreeMap::new()).expect("resolve");
        assert_eq!(
            resolved.get("env"),
            Some(&ParameterValue::String("prod".into()))
        );
        assert_eq!(
            resolved.get("service"),
            Some(&ParameterValue::String("api".into()))
        );
    }

    #[test]
    fn referenced_names_walks_deeply() {
        let value = json!({
            "url": "https://api/$env/v1",
            "args": ["$project", "${service}.id", "literal"],
            "nested": { "from": "$range.from" }
        });
        let refs = ResolvedParameters::referenced_names(&value);
        assert!(refs.contains("env"));
        assert!(refs.contains("project"));
        assert!(refs.contains("service"));
        assert!(refs.contains("range"));
    }

    #[test]
    fn dollar_dollar_is_escape() {
        let resolved = ResolvedParameters::new();
        let out = resolved.substitute_value(&json!("price $$5"), SubstituteOptions::default());
        assert_eq!(out, json!("price $$5"));
    }
}

impl PartialEq for ParameterValue {
    fn eq(&self, other: &Self) -> bool {
        use ParameterValue::*;
        match (self, other) {
            (String(a), String(b)) => a == b,
            (Number(a), Number(b)) => (a - b).abs() < f64::EPSILON,
            (Bool(a), Bool(b)) => a == b,
            (Range(a), Range(b)) => a.from == b.from && a.to == b.to,
            (Array(a), Array(b)) => a == b,
            _ => false,
        }
    }
}

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tracing::{error, info};

use crate::models::chat::{ChatMessage, ChatMessagePart, ChatMode, MessageRole};
use crate::models::provider::LLMProvider;
use crate::models::workflow::{
    NodeKind, RunStatus, TriggerKind, Workflow, WorkflowEdge, WorkflowEventEnvelope,
    WorkflowEventKind, WorkflowNode, WorkflowRun, WorkflowTrigger,
};
use crate::models::Id;
use crate::modules::ai::AIEngine;
use crate::modules::mcp_manager::MCPManager;
use crate::modules::tool_engine::ToolEngine;

pub struct WorkflowExecution {
    pub run: WorkflowRun,
    pub events: Vec<WorkflowEventEnvelope>,
}

/// DAG-based workflow execution engine
pub struct WorkflowEngine<'a> {
    tool_engine: Option<&'a ToolEngine>,
    mcp_manager: Option<&'a MCPManager>,
    ai_engine: Option<&'a AIEngine>,
    provider: Option<LLMProvider>,
}

impl<'a> WorkflowEngine<'a> {
    pub fn new() -> Self {
        Self {
            tool_engine: None,
            mcp_manager: None,
            ai_engine: None,
            provider: None,
        }
    }

    pub fn with_tool_engine(tool_engine: &'a ToolEngine) -> Self {
        Self {
            tool_engine: Some(tool_engine),
            mcp_manager: None,
            ai_engine: None,
            provider: None,
        }
    }

    pub fn with_runtime(
        tool_engine: &'a ToolEngine,
        mcp_manager: &'a MCPManager,
        ai_engine: &'a AIEngine,
        provider: Option<LLMProvider>,
    ) -> Self {
        Self {
            tool_engine: Some(tool_engine),
            mcp_manager: Some(mcp_manager),
            ai_engine: Some(ai_engine),
            provider,
        }
    }

    /// Execute a workflow by ID
    pub async fn execute(
        &self,
        workflow: &Workflow,
        input: Option<Value>,
    ) -> Result<WorkflowExecution> {
        let run_id = uuid::Uuid::new_v4().to_string();
        let start_time = chrono::Utc::now().timestamp_millis();
        let mut events = vec![Self::event(
            WorkflowEventKind::RunStarted,
            workflow,
            &run_id,
            None,
            RunStatus::Running,
            input.clone(),
            None,
        )];

        info!("⚡ Executing workflow '{}' ({})", workflow.name, run_id);

        // Build execution DAG
        let execution_order = Self::topological_sort(&workflow.nodes, &workflow.edges)?;

        // Execution context: node_id -> result
        let mut context: HashMap<String, Value> = HashMap::new();

        // Inject input if provided
        if let Some(inp) = input {
            context.insert("__input".to_string(), inp);
        }

        // Execute nodes in topological order
        for node_id in &execution_order {
            let node = workflow
                .nodes
                .iter()
                .find(|n| &n.id == node_id)
                .ok_or_else(|| anyhow!("Node {} not found", node_id))?;

            events.push(Self::event(
                WorkflowEventKind::NodeStarted,
                workflow,
                &run_id,
                Some(node_id.clone()),
                RunStatus::Running,
                None,
                None,
            ));

            match self.execute_node(node, &context).await {
                Ok(result) => {
                    events.push(Self::event(
                        WorkflowEventKind::NodeFinished,
                        workflow,
                        &run_id,
                        Some(node_id.clone()),
                        RunStatus::Success,
                        Some(result.clone()),
                        None,
                    ));
                    context.insert(node_id.clone(), result);
                }
                Err(e) => {
                    error!("❌ Node '{}' failed: {}", node.label, e);
                    let run = WorkflowRun {
                        id: run_id,
                        started_at: start_time,
                        finished_at: Some(chrono::Utc::now().timestamp_millis()),
                        status: RunStatus::Error,
                        node_results: Some(serde_json::to_value(&context)?),
                        error: Some(e.to_string()),
                    };
                    events.push(Self::event(
                        WorkflowEventKind::NodeFinished,
                        workflow,
                        &run.id,
                        Some(node_id.clone()),
                        RunStatus::Error,
                        None,
                        run.error.clone(),
                    ));
                    events.push(Self::event(
                        WorkflowEventKind::RunFinished,
                        workflow,
                        &run.id,
                        None,
                        RunStatus::Error,
                        run.node_results.clone(),
                        run.error.clone(),
                    ));
                    return Ok(WorkflowExecution { run, events });
                }
            }
        }

        let finish_time = chrono::Utc::now().timestamp_millis();
        info!(
            "✅ Workflow '{}' completed in {}ms",
            workflow.name,
            finish_time - start_time
        );

        let run = WorkflowRun {
            id: run_id,
            started_at: start_time,
            finished_at: Some(finish_time),
            status: RunStatus::Success,
            node_results: Some(serde_json::to_value(&context)?),
            error: None,
        };
        events.push(Self::event(
            WorkflowEventKind::RunFinished,
            workflow,
            &run.id,
            None,
            RunStatus::Success,
            run.node_results.clone(),
            None,
        ));

        Ok(WorkflowExecution { run, events })
    }

    /// Execute a single node
    async fn execute_node(
        &self,
        node: &WorkflowNode,
        context: &HashMap<String, Value>,
    ) -> Result<Value> {
        let empty_config = json!({});
        let config = node.config.as_ref().unwrap_or(&empty_config);
        match node.kind {
            NodeKind::McpTool => {
                let server_id = config
                    .get("server_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing server_id"))?;
                let tool_name = config
                    .get("tool_name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing tool_name"))?;
                if let Some(tool_engine) = self.tool_engine {
                    tool_engine.validate_mcp_tool_call(server_id, tool_name)?;
                }

                if server_id == "builtin" {
                    return self.execute_builtin_tool(tool_name, config).await;
                }

                let mcp_manager = self
                    .mcp_manager
                    .ok_or_else(|| anyhow!("MCP runtime is unavailable for workflow execution"))?;
                let raw = mcp_manager
                    .call_tool(server_id, tool_name, config.get("arguments").cloned())
                    .await?;
                // Stdio MCP servers wrap payloads as {"content":[{"text":"<json>"}]}.
                // Unwrap and parse at the source so downstream pipeline steps
                // see the actual data shape instead of having to navigate the
                // wrapper themselves.
                Ok(mcp_unwrap_content(&raw).unwrap_or(raw))
            }

            NodeKind::Llm => {
                let ai_engine = self
                    .ai_engine
                    .ok_or_else(|| anyhow!("AI runtime is unavailable for workflow execution"))?;
                let provider = self.provider.as_ref().ok_or_else(|| {
                    anyhow!("Workflow LLM node requires an enabled active provider")
                })?;
                let prompt = config
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Summarize the workflow context.");
                let input_key = config.get("input_key").and_then(|v| v.as_str());
                let grounded_input = input_key
                    .and_then(|key| context.get(key))
                    .cloned()
                    .unwrap_or_else(|| serde_json::to_value(context).unwrap_or(json!({})));
                let messages = vec![
                    runtime_chat_message(MessageRole::System, "You are executing a Datrina workflow LLM node. Return concise content for downstream workflow nodes."),
                    runtime_chat_message(
                        MessageRole::User,
                        &format!("Prompt: {}\nWorkflow context JSON: {}", prompt, grounded_input),
                    ),
                ];
                let response = ai_engine.complete_chat(provider, &messages).await?;
                Ok(json!({
                    "content": response.content,
                    "provider_id": response.provider_id,
                    "model": response.model,
                    "tokens": response.tokens,
                    "latency_ms": response.latency_ms,
                }))
            }

            NodeKind::Transform => {
                let input_key = config
                    .get("input_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("__input");
                let transform = config
                    .get("transform")
                    .and_then(|v| v.as_str())
                    .unwrap_or("identity");

                let input_data = context.get(input_key).cloned().unwrap_or(json!({}));

                info!("🔄 Transform: {} -> {}", input_key, transform);
                match transform {
                    "identity" => Ok(input_data),
                    "pick" => {
                        let key = config
                            .get("key")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Transform 'pick' requires key"))?;
                        Ok(input_data.get(key).cloned().unwrap_or(Value::Null))
                    }
                    "pick_path" => {
                        let path = config
                            .get("path")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Transform 'pick_path' requires path"))?;
                        Ok(pick_path(&input_data, path).cloned().unwrap_or(Value::Null))
                    }
                    "pipeline" => {
                        let steps_value = config
                            .get("steps")
                            .ok_or_else(|| anyhow!("Transform 'pipeline' requires steps"))?;
                        let steps: Vec<crate::models::pipeline::PipelineStep> =
                            serde_json::from_value(steps_value.clone())
                                .map_err(|e| anyhow!("Invalid pipeline steps: {}", e))?;
                        run_pipeline(input_data, &steps, self.ai_engine, self.provider.as_ref())
                            .await
                    }
                    other => Err(anyhow!("Unsupported transform '{}'", other)),
                }
            }

            NodeKind::Datasource => {
                let data = config.get("data").cloned().unwrap_or(json!({}));
                info!("📊 Datasource loaded");
                Ok(data)
            }

            NodeKind::Condition => {
                let expression = config
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("true");
                info!("❓ Condition: {}", expression);
                match expression {
                    "true" => Ok(json!({ "condition": expression, "result": true })),
                    "false" => Ok(json!({ "condition": expression, "result": false })),
                    other => Err(anyhow!("Unsupported condition expression '{}'", other)),
                }
            }

            NodeKind::Merge => {
                let keys = config
                    .get("keys")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();

                let mut merged = serde_json::Map::new();
                for key in &keys {
                    if let Some(val) = context.get(key) {
                        merged.insert(key.clone(), val.clone());
                    }
                }
                info!("🔗 Merged {} keys", merged.len());
                Ok(Value::Object(merged))
            }

            NodeKind::Output => {
                let output_key = config
                    .get("output_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("result");
                let input_node = config.get("input_node").and_then(|v| v.as_str());

                let value = match input_node {
                    Some(node_id) => context.get(node_id).cloned().unwrap_or(json!({})),
                    None => {
                        let mut map = serde_json::Map::new();
                        for (k, v) in context.iter() {
                            if !k.starts_with("__") {
                                map.insert(k.clone(), v.clone());
                            }
                        }
                        Value::Object(map)
                    }
                };

                let mut output = serde_json::Map::new();
                output.insert(output_key.to_string(), value);
                Ok(Value::Object(output))
            }
        }
    }

    async fn execute_builtin_tool(&self, tool_name: &str, config: &Value) -> Result<Value> {
        let tool_engine = self
            .tool_engine
            .ok_or_else(|| anyhow!("Tool runtime is unavailable for workflow execution"))?;
        match tool_name {
            "http_request" => {
                let arguments = config.get("arguments").unwrap_or(config);
                let method = arguments
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or("GET");
                let url = arguments
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("http_request requires url"))?;
                tool_engine
                    .http_request(
                        method,
                        url,
                        arguments.get("body").cloned(),
                        arguments.get("headers").cloned(),
                    )
                    .await
            }
            "curl" => {
                let args = config
                    .get("arguments")
                    .and_then(|v| v.get("args"))
                    .or_else(|| config.get("args"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("curl workflow tool requires args"))?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(ToString::to_string)
                            .ok_or_else(|| anyhow!("curl args must be strings"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                tool_engine.execute_curl(args).await
            }
            other => Err(anyhow!("Unsupported built-in workflow tool '{}'", other)),
        }
    }

    fn event(
        kind: WorkflowEventKind,
        workflow: &Workflow,
        run_id: &str,
        node_id: Option<Id>,
        status: RunStatus,
        payload: Option<Value>,
        error: Option<String>,
    ) -> WorkflowEventEnvelope {
        WorkflowEventEnvelope {
            kind,
            workflow_id: workflow.id.clone(),
            run_id: run_id.to_string(),
            node_id,
            status,
            payload,
            error,
            emitted_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Topological sort of workflow nodes
    fn topological_sort(nodes: &[WorkflowNode], edges: &[WorkflowEdge]) -> Result<Vec<Id>> {
        // Build adjacency list and in-degree map
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut in_degree: HashMap<&str, usize> = HashMap::new();

        // Initialize all nodes with in-degree 0
        for node in nodes {
            in_degree.entry(&node.id).or_insert(0);
            adjacency.entry(&node.id).or_default();
        }

        // Build edges
        for edge in edges {
            adjacency
                .entry(&edge.source)
                .or_default()
                .push(&edge.target);
            *in_degree.entry(&edge.target).or_insert(0) += 1;
        }

        // Kahn's algorithm
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut result = Vec::new();

        while let Some(node_id) = queue.pop_front() {
            result.push(node_id.to_string());

            if let Some(neighbors) = adjacency.get(node_id) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        // Check for cycles
        if result.len() != nodes.len() {
            return Err(anyhow!("Workflow contains a cycle"));
        }

        Ok(result)
    }

    /// Check if a trigger should fire (for event triggers)
    pub fn should_trigger(&self, trigger: &WorkflowTrigger, event_name: &str) -> bool {
        match trigger.kind {
            TriggerKind::Event => trigger
                .config
                .as_ref()
                .and_then(|c| c.event.as_ref())
                .map(|e| e == event_name)
                .unwrap_or(false),
            _ => false,
        }
    }
}

fn pick_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.').filter(|segment| !segment.is_empty()) {
        if let Ok(index) = segment.parse::<usize>() {
            current = current.as_array()?.get(index)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current)
}

/// Resolve a dotted path with `[index]` and `[*]` segments. Always returns
/// an owned `Value`; `[*]` flattens an array into a Vec of matched values.
pub(crate) fn resolve_path(value: &Value, path: &str) -> Value {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return value.clone();
    }
    let segments = split_path_segments(trimmed);
    resolve_segments(value, &segments)
}

fn split_path_segments(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '.' => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
            }
            '[' => {
                if !buf.is_empty() {
                    out.push(std::mem::take(&mut buf));
                }
                let mut idx = String::new();
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c == ']' {
                        break;
                    }
                    idx.push(c);
                }
                out.push(format!("[{}]", idx));
            }
            other => buf.push(other),
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

fn resolve_segments(value: &Value, segments: &[String]) -> Value {
    if segments.is_empty() {
        return value.clone();
    }
    let (head, rest) = segments.split_first().unwrap();
    if head == "[*]" {
        match value {
            Value::Array(items) => {
                let collected: Vec<Value> = items
                    .iter()
                    .map(|item| resolve_segments(item, rest))
                    .collect();
                // If `rest` contains another `[*]` we end up with an array
                // of arrays. Flatten one level so chained wildcards behave
                // like JMESPath flattening (`[*].issues[*]` -> flat list).
                let needs_flatten = rest.iter().any(|s| s == "[*]");
                if needs_flatten {
                    let mut flat = Vec::with_capacity(collected.len());
                    for item in collected {
                        match item {
                            Value::Array(nested) => flat.extend(nested),
                            other => flat.push(other),
                        }
                    }
                    Value::Array(flat)
                } else {
                    Value::Array(collected)
                }
            }
            _ => Value::Null,
        }
    } else if let Some(idx_str) = head.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        if let Ok(idx) = idx_str.parse::<usize>() {
            match value.as_array().and_then(|arr| arr.get(idx)) {
                Some(next) => resolve_segments(next, rest),
                None => Value::Null,
            }
        } else {
            Value::Null
        }
    } else if let Ok(idx) = head.parse::<usize>() {
        match value.as_array().and_then(|arr| arr.get(idx)) {
            Some(next) => resolve_segments(next, rest),
            None => Value::Null,
        }
    } else {
        match value.get(head) {
            Some(next) => resolve_segments(next, rest),
            None => Value::Null,
        }
    }
}

async fn run_pipeline(
    input: Value,
    steps: &[crate::models::pipeline::PipelineStep],
    ai_engine: Option<&crate::modules::ai::AIEngine>,
    provider: Option<&crate::models::provider::LLMProvider>,
) -> Result<Value> {
    let mut current = input;
    for step in steps {
        current = apply_pipeline_step(current, step, ai_engine, provider).await?;
    }
    Ok(current)
}

/// W23: trace-instrumented pipeline runner. Returns the final value and a
/// per-step trace capturing pruned input/output samples, durations, and
/// (if applicable) the step error. The normal `run_pipeline` path keeps
/// its zero-overhead signature.
pub(crate) async fn run_pipeline_with_trace(
    input: Value,
    steps: &[crate::models::pipeline::PipelineStep],
    ai_engine: Option<&crate::modules::ai::AIEngine>,
    provider: Option<&crate::models::provider::LLMProvider>,
) -> (Value, Vec<crate::models::pipeline::PipelineStepTrace>) {
    let mut current = input;
    let mut traces = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let input_sample = prune_for_trace(&current);
        let started = std::time::Instant::now();
        let kind = pipeline_step_kind(step);
        let config_json = serde_json::to_value(step).unwrap_or(Value::Null);
        match apply_pipeline_step(current.clone(), step, ai_engine, provider).await {
            Ok(next) => {
                let duration_ms = started.elapsed().as_millis() as u32;
                let output_sample = prune_for_trace(&next);
                traces.push(crate::models::pipeline::PipelineStepTrace {
                    index: index as u32,
                    kind,
                    config_json,
                    input_sample,
                    output_sample,
                    duration_ms,
                    error: None,
                });
                current = next;
            }
            Err(error) => {
                let duration_ms = started.elapsed().as_millis() as u32;
                traces.push(crate::models::pipeline::PipelineStepTrace {
                    index: index as u32,
                    kind,
                    config_json,
                    input_sample,
                    output_sample: prune_for_trace(&Value::Null),
                    duration_ms,
                    error: Some(error.to_string()),
                });
                return (Value::Null, traces);
            }
        }
    }
    (current, traces)
}

fn pipeline_step_kind(step: &crate::models::pipeline::PipelineStep) -> String {
    use crate::models::pipeline::PipelineStep::*;
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
    .to_string()
}

/// Build a [`SampleValue`] from an arbitrary JSON value with strict size
/// caps so traces are safe to store and serialize: strings >256 chars
/// truncated, arrays >5 items kept as head, depth >5 collapsed.
pub(crate) fn prune_for_trace(value: &Value) -> crate::models::pipeline::SampleValue {
    use crate::models::pipeline::{SampleKind, SampleValue, SizeHint};
    const MAX_STR: usize = 256;
    const MAX_DEPTH: usize = 5;
    let preview = prune_value(value, MAX_DEPTH);
    match value {
        Value::Null => SampleValue {
            kind: SampleKind::Null,
            size_hint: SizeHint::default(),
            preview,
        },
        Value::Array(items) => SampleValue {
            kind: SampleKind::ArrayHead,
            size_hint: SizeHint {
                items: Some(items.len()),
                bytes: None,
            },
            preview,
        },
        Value::Object(map) => SampleValue {
            kind: SampleKind::Object,
            size_hint: SizeHint {
                items: Some(map.len()),
                bytes: None,
            },
            preview,
        },
        Value::String(s) => {
            if s.chars().count() > MAX_STR {
                SampleValue {
                    kind: SampleKind::TruncatedString,
                    size_hint: SizeHint {
                        items: None,
                        bytes: Some(s.len()),
                    },
                    preview,
                }
            } else {
                SampleValue {
                    kind: SampleKind::Value,
                    size_hint: SizeHint {
                        items: None,
                        bytes: Some(s.len()),
                    },
                    preview,
                }
            }
        }
        _ => SampleValue {
            kind: SampleKind::Value,
            size_hint: SizeHint::default(),
            preview,
        },
    }
}

fn prune_value(value: &Value, depth_remaining: usize) -> Value {
    const MAX_STR: usize = 256;
    const MAX_ARR: usize = 5;
    if depth_remaining == 0 {
        return Value::String(match value {
            Value::Array(items) => format!("[…{} items]", items.len()),
            Value::Object(map) => format!("{{…{} keys}}", map.len()),
            other => other.to_string(),
        });
    }
    match value {
        Value::String(s) => {
            if s.chars().count() > MAX_STR {
                let truncated: String = s.chars().take(MAX_STR).collect();
                Value::String(format!(
                    "{}… [{} chars total]",
                    truncated,
                    s.chars().count()
                ))
            } else {
                Value::String(s.clone())
            }
        }
        Value::Array(items) => {
            let mut head: Vec<Value> = items
                .iter()
                .take(MAX_ARR)
                .map(|item| prune_value(item, depth_remaining - 1))
                .collect();
            if items.len() > MAX_ARR {
                head.push(Value::String(format!("… {} more", items.len() - MAX_ARR)));
            }
            Value::Array(head)
        }
        Value::Object(map) => {
            let mut pruned = serde_json::Map::new();
            for (k, v) in map.iter() {
                pruned.insert(k.clone(), prune_value(v, depth_remaining - 1));
            }
            Value::Object(pruned)
        }
        other => other.clone(),
    }
}

async fn apply_pipeline_step(
    current: Value,
    step: &crate::models::pipeline::PipelineStep,
    ai_engine: Option<&crate::modules::ai::AIEngine>,
    provider: Option<&crate::models::provider::LLMProvider>,
) -> Result<Value> {
    use crate::models::pipeline::{LlmExpect, PipelineStep, SortOrder};
    match step {
        PipelineStep::Pick { path } => Ok(resolve_path(&current, path)),
        PipelineStep::Filter { field, op, value } => {
            let arr = current.as_array().cloned().unwrap_or_default();
            let kept: Vec<Value> = arr
                .into_iter()
                .filter(|item| filter_predicate(item, field, op, value))
                .collect();
            Ok(Value::Array(kept))
        }
        PipelineStep::Sort { by, order } => {
            let mut arr = current.as_array().cloned().unwrap_or_default();
            arr.sort_by(|a, b| {
                let av = resolve_path(a, by);
                let bv = resolve_path(b, by);
                compare_path_values(&av, &bv)
            });
            if matches!(order, SortOrder::Desc) {
                arr.reverse();
            }
            Ok(Value::Array(arr))
        }
        PipelineStep::Limit { count } => {
            let arr = current.as_array().cloned().unwrap_or_default();
            Ok(Value::Array(arr.into_iter().take(*count).collect()))
        }
        PipelineStep::Map { fields, rename } => {
            let arr = current.as_array().cloned().unwrap_or_default();
            let mapped: Vec<Value> = arr
                .into_iter()
                .map(|item| map_item(&item, fields, rename))
                .collect();
            Ok(Value::Array(mapped))
        }
        PipelineStep::Aggregate {
            group_by,
            metric,
            output_key,
        } => {
            let arr = current.as_array().cloned().unwrap_or_default();
            if let Some(group_field) = group_by {
                let mut groups: std::collections::BTreeMap<String, Vec<Value>> =
                    std::collections::BTreeMap::new();
                for item in arr {
                    let key_val = resolve_path(&item, group_field);
                    let key = match &key_val {
                        Value::Null => String::new(),
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    groups.entry(key).or_default().push(item);
                }
                let rows: Vec<Value> = groups
                    .into_iter()
                    .map(|(group, items)| {
                        let mut row = serde_json::Map::new();
                        row.insert(group_field.clone(), Value::String(group));
                        row.insert(output_key.clone(), aggregate_metric(&items, metric));
                        Value::Object(row)
                    })
                    .collect();
                Ok(Value::Array(rows))
            } else {
                let mut obj = serde_json::Map::new();
                obj.insert(output_key.clone(), aggregate_metric(&arr, metric));
                Ok(Value::Object(obj))
            }
        }
        PipelineStep::Set { field, value } => {
            let mut obj = current.as_object().cloned().unwrap_or_default();
            obj.insert(field.clone(), value.clone());
            Ok(Value::Object(obj))
        }
        PipelineStep::Head => Ok(match current {
            Value::Array(items) => items.into_iter().next().unwrap_or(Value::Null),
            other => other,
        }),
        PipelineStep::Tail => Ok(match current {
            Value::Array(items) => items.into_iter().last().unwrap_or(Value::Null),
            other => other,
        }),
        PipelineStep::Length => Ok(match &current {
            Value::Array(items) => Value::from(items.len()),
            Value::Object(map) => Value::from(map.len()),
            Value::String(s) => Value::from(s.chars().count()),
            Value::Null => Value::from(0),
            _ => Value::from(1),
        }),
        PipelineStep::Flatten => Ok(match current {
            Value::Array(items) => {
                let mut flat = Vec::new();
                for item in items {
                    if let Value::Array(nested) = item {
                        flat.extend(nested);
                    } else {
                        flat.push(item);
                    }
                }
                Value::Array(flat)
            }
            other => other,
        }),
        PipelineStep::Unique { by } => Ok(match current {
            Value::Array(items) => {
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                let mut kept = Vec::new();
                for item in items {
                    let key = match by {
                        Some(field) => item.get(field).cloned().unwrap_or(Value::Null).to_string(),
                        None => item.to_string(),
                    };
                    if seen.insert(key) {
                        kept.push(item);
                    }
                }
                Value::Array(kept)
            }
            other => other,
        }),
        PipelineStep::Format {
            template,
            output_key,
        } => {
            let render_one = |scope: &Value| -> String { render_template(template, scope) };
            let formatted = match &current {
                Value::Array(items) => Value::Array(
                    items
                        .iter()
                        .map(|item| Value::String(render_one(item)))
                        .collect(),
                ),
                _ => Value::String(render_one(&current)),
            };
            if let Some(key) = output_key {
                let mut obj = current.as_object().cloned().unwrap_or_default();
                obj.insert(key.clone(), formatted);
                Ok(Value::Object(obj))
            } else {
                Ok(formatted)
            }
        }
        PipelineStep::Coerce { to } => Ok(coerce_value(&current, to)),
        PipelineStep::LlmPostprocess { prompt, expect } => {
            let engine =
                ai_engine.ok_or_else(|| anyhow!("LlmPostprocess requires an active provider"))?;
            let provider =
                provider.ok_or_else(|| anyhow!("LlmPostprocess requires an active provider"))?;
            let system = match expect {
                LlmExpect::Text => "You are a deterministic data postprocessor in a Datrina workflow pipeline. Read the JSON input and produce a CONCISE human-readable answer per the prompt. Respond with plain text or markdown; never wrap your answer in JSON.",
                LlmExpect::Json => "You are a deterministic data postprocessor in a Datrina workflow pipeline. Read the JSON input and produce STRICT JSON per the prompt. Do not include markdown fences or commentary.",
            };
            let user = format!(
                "Prompt: {}\nInput JSON:\n{}",
                prompt,
                serde_json::to_string_pretty(&current).unwrap_or_else(|_| current.to_string())
            );
            let messages = vec![
                runtime_chat_message(MessageRole::System, system),
                runtime_chat_message(MessageRole::User, &user),
            ];
            let response = engine.complete_chat(provider, &messages).await?;
            match expect {
                LlmExpect::Text => Ok(Value::String(response.content)),
                LlmExpect::Json => serde_json::from_str(&response.content)
                    .map_err(|e| anyhow!("LlmPostprocess: response was not valid JSON: {}", e)),
            }
        }
    }
}

fn filter_predicate(
    item: &Value,
    field: &str,
    op: &crate::models::pipeline::FilterOp,
    value: &Value,
) -> bool {
    use crate::models::pipeline::FilterOp::*;
    let field_value = if field.is_empty() {
        item.clone()
    } else {
        resolve_path(item, field)
    };
    match op {
        Eq => &field_value == value,
        Ne => &field_value != value,
        Gt | Gte | Lt | Lte => {
            let lhs = field_value.as_f64();
            let rhs = value.as_f64();
            match (lhs, rhs) {
                (Some(a), Some(b)) => match op {
                    Gt => a > b,
                    Gte => a >= b,
                    Lt => a < b,
                    Lte => a <= b,
                    _ => false,
                },
                _ => false,
            }
        }
        Contains => match (&field_value, value) {
            (Value::String(s), Value::String(needle)) => s.contains(needle.as_str()),
            (Value::Array(items), needle) => items.iter().any(|i| i == needle),
            _ => false,
        },
        StartsWith => match (&field_value, value) {
            (Value::String(s), Value::String(p)) => s.starts_with(p.as_str()),
            _ => false,
        },
        EndsWith => match (&field_value, value) {
            (Value::String(s), Value::String(p)) => s.ends_with(p.as_str()),
            _ => false,
        },
        In => value
            .as_array()
            .map(|arr| arr.iter().any(|v| v == &field_value))
            .unwrap_or(false),
        NotIn => !value
            .as_array()
            .map(|arr| arr.iter().any(|v| v == &field_value))
            .unwrap_or(false),
        Exists => !field_value.is_null(),
        NotExists => field_value.is_null(),
        Truthy => is_truthy(&field_value),
        Falsy => !is_truthy(&field_value),
    }
}

fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|x| x != 0.0).unwrap_or(false),
        Value::String(s) => !s.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

fn compare_path_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::Number(an), Value::Number(bn)) => an
            .as_f64()
            .partial_cmp(&bn.as_f64())
            .unwrap_or(Ordering::Equal),
        (Value::String(an), Value::String(bn)) => {
            // If both look like numbers, compare numerically; otherwise lexicographic.
            if let (Ok(af), Ok(bf)) = (an.parse::<f64>(), bn.parse::<f64>()) {
                af.partial_cmp(&bf).unwrap_or(Ordering::Equal)
            } else {
                an.cmp(bn)
            }
        }
        _ => a.to_string().cmp(&b.to_string()),
    }
}

fn compare_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(av), Some(bv)) => match (av, bv) {
            (Value::Number(an), Value::Number(bn)) => an
                .as_f64()
                .partial_cmp(&bn.as_f64())
                .unwrap_or(Ordering::Equal),
            (Value::String(an), Value::String(bn)) => an.cmp(bn),
            _ => av.to_string().cmp(&bv.to_string()),
        },
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn map_item(
    item: &Value,
    fields: &[String],
    rename: &std::collections::BTreeMap<String, String>,
) -> Value {
    let mut next = serde_json::Map::new();
    if fields.is_empty() && rename.is_empty() {
        return item.clone();
    }
    let source = item.as_object().cloned().unwrap_or_default();
    let take_keys: Vec<String> = if fields.is_empty() {
        source.keys().map(|k| k.to_string()).collect()
    } else {
        fields.to_vec()
    };
    for key in take_keys {
        let value = resolve_path(item, &key);
        let target_key = rename.get(&key).cloned().unwrap_or_else(|| key.clone());
        next.insert(target_key, value);
    }
    Value::Object(next)
}

fn mcp_unwrap_content(value: &Value) -> Option<Value> {
    let content = value.get("content")?.as_array()?;
    let text_parts: Vec<&str> = content
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect();
    if text_parts.is_empty() {
        return None;
    }
    let text = text_parts.join("\n");
    Some(serde_json::from_str::<Value>(&text).unwrap_or_else(|_| Value::String(text)))
}

fn render_template(template: &str, scope: &Value) -> String {
    let mut out = String::with_capacity(template.len());
    let mut iter = template.chars().peekable();
    while let Some(ch) = iter.next() {
        if ch == '{' {
            let mut name = String::new();
            let mut closed = false;
            while let Some(&next) = iter.peek() {
                iter.next();
                if next == '}' {
                    closed = true;
                    break;
                }
                name.push(next);
            }
            if closed {
                let value = resolve_path(scope, name.trim());
                let rendered = match value {
                    Value::String(s) => s,
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                out.push_str(&rendered);
            } else {
                out.push('{');
                out.push_str(&name);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn coerce_value(value: &Value, target: &crate::models::pipeline::CoerceTarget) -> Value {
    use crate::models::pipeline::CoerceTarget;
    match target {
        CoerceTarget::Number => match value {
            Value::Number(_) => value.clone(),
            Value::String(s) => s
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(|f| serde_json::Number::from_f64(f).map(Value::Number))
                .unwrap_or(Value::Null),
            Value::Bool(b) => Value::from(if *b { 1 } else { 0 }),
            Value::Null => Value::Null,
            Value::Array(items) => Value::from(items.len()),
            Value::Object(map) => Value::from(map.len()),
        },
        CoerceTarget::Integer => match value {
            Value::Number(n) => n.as_i64().map(Value::from).unwrap_or_else(|| {
                n.as_f64()
                    .map(|f| Value::from(f.trunc() as i64))
                    .unwrap_or(Value::Null)
            }),
            Value::String(s) => s
                .trim()
                .parse::<i64>()
                .ok()
                .map(Value::from)
                .unwrap_or(Value::Null),
            Value::Bool(b) => Value::from(if *b { 1 } else { 0 }),
            Value::Null => Value::Null,
            Value::Array(items) => Value::from(items.len() as i64),
            Value::Object(map) => Value::from(map.len() as i64),
        },
        CoerceTarget::String => match value {
            Value::String(_) => value.clone(),
            Value::Null => Value::String(std::string::String::new()),
            other => Value::String(other.to_string()),
        },
        CoerceTarget::Array => match value {
            Value::Array(_) => value.clone(),
            Value::Null => Value::Array(Vec::new()),
            other => Value::Array(vec![other.clone()]),
        },
    }
}

fn aggregate_metric(items: &[Value], metric: &crate::models::pipeline::AggregateMetric) -> Value {
    use crate::models::pipeline::AggregateMetric::*;
    let pick = |item: &Value, field: &str| -> Value { resolve_path(item, field) };
    let pick_num = |item: &Value, field: &str| -> Option<f64> {
        let v = resolve_path(item, field);
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    };
    match metric {
        Count => serde_json::json!(items.len()),
        Sum { field } => {
            let total: f64 = items.iter().filter_map(|item| pick_num(item, field)).sum();
            serde_json::json!(total)
        }
        Avg { field } => {
            let values: Vec<f64> = items
                .iter()
                .filter_map(|item| pick_num(item, field))
                .collect();
            if values.is_empty() {
                Value::Null
            } else {
                let sum: f64 = values.iter().sum();
                serde_json::json!(sum / values.len() as f64)
            }
        }
        Min { field } => items
            .iter()
            .filter_map(|item| pick_num(item, field))
            .reduce(f64::min)
            .map(|v| serde_json::json!(v))
            .unwrap_or(Value::Null),
        Max { field } => items
            .iter()
            .filter_map(|item| pick_num(item, field))
            .reduce(f64::max)
            .map(|v| serde_json::json!(v))
            .unwrap_or(Value::Null),
        First { field } => items
            .first()
            .map(|item| pick(item, field))
            .unwrap_or(Value::Null),
        Last { field } => items
            .last()
            .map(|item| pick(item, field))
            .unwrap_or(Value::Null),
    }
}

impl<'a> Default for WorkflowEngine<'a> {
    fn default() -> Self {
        Self::new()
    }
}

fn runtime_chat_message(role: MessageRole, content: &str) -> ChatMessage {
    ChatMessage {
        id: uuid::Uuid::new_v4().to_string(),
        role,
        content: content.to_string(),
        parts: if content.trim().is_empty() {
            Vec::new()
        } else {
            vec![ChatMessagePart::Text {
                text: content.to_string(),
            }]
        },
        mode: ChatMode::Context,
        tool_calls: None,
        tool_results: None,
        metadata: None,
        timestamp: chrono::Utc::now().timestamp_millis(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::workflow::{
        NodeKind, TriggerKind, WorkflowEdge, WorkflowNode, WorkflowTrigger,
    };

    fn workflow(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> Workflow {
        let now = chrono::Utc::now().timestamp_millis();
        Workflow {
            id: "workflow-1".into(),
            name: "Local workflow".into(),
            description: None,
            nodes,
            edges,
            trigger: WorkflowTrigger {
                kind: TriggerKind::Manual,
                config: None,
            },
            is_enabled: true,
            last_run: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn executes_deterministic_local_nodes_and_records_events() -> Result<()> {
        let source = WorkflowNode {
            id: "source".into(),
            kind: NodeKind::Datasource,
            label: "Source".into(),
            position: None,
            config: Some(json!({ "data": { "value": 42, "label": "ok" } })),
        };
        let pick = WorkflowNode {
            id: "pick".into(),
            kind: NodeKind::Transform,
            label: "Pick value".into(),
            position: None,
            config: Some(json!({
                "input_key": "source",
                "transform": "pick",
                "key": "value"
            })),
        };
        let output = WorkflowNode {
            id: "output".into(),
            kind: NodeKind::Output,
            label: "Output".into(),
            position: None,
            config: Some(json!({ "input_node": "pick", "output_key": "result" })),
        };
        let wf = workflow(
            vec![source, pick, output],
            vec![
                WorkflowEdge {
                    id: "edge-1".into(),
                    source: "source".into(),
                    target: "pick".into(),
                    condition: None,
                },
                WorkflowEdge {
                    id: "edge-2".into(),
                    source: "pick".into(),
                    target: "output".into(),
                    condition: None,
                },
            ],
        );

        let execution = WorkflowEngine::new().execute(&wf, None).await?;

        assert!(matches!(execution.run.status, RunStatus::Success));
        assert_eq!(
            execution
                .run
                .node_results
                .as_ref()
                .and_then(|v| v.get("output"))
                .and_then(|v| v.get("result")),
            Some(&json!(42))
        );
        assert!(execution
            .events
            .iter()
            .any(|event| matches!(event.kind, WorkflowEventKind::RunStarted)));
        assert!(execution
            .events
            .iter()
            .any(|event| matches!(event.kind, WorkflowEventKind::RunFinished)));

        Ok(())
    }

    #[test]
    fn prune_for_trace_truncates_strings_and_arrays() {
        use crate::models::pipeline::SampleKind;
        // Long string → truncated_string kind, preview ends with `…`.
        let long = "x".repeat(500);
        let sample = prune_for_trace(&Value::String(long.clone()));
        assert!(matches!(sample.kind, SampleKind::TruncatedString));
        let preview = sample.preview.as_str().unwrap();
        assert!(preview.contains("[500 chars total]"));
        assert!(preview.starts_with("x"));

        // Long array → array_head, only first 5 items kept + tail marker.
        let arr: Vec<Value> = (0..20).map(|i| Value::from(i)).collect();
        let sample = prune_for_trace(&Value::Array(arr));
        assert!(matches!(sample.kind, SampleKind::ArrayHead));
        assert_eq!(sample.size_hint.items, Some(20));
        let preview_items = sample.preview.as_array().unwrap();
        assert_eq!(preview_items.len(), 6); // 5 items + "... N more" marker
        assert_eq!(preview_items[0], Value::from(0));
        assert!(preview_items[5].as_str().unwrap().contains("15 more"));

        // Null → null kind.
        let sample = prune_for_trace(&Value::Null);
        assert!(matches!(sample.kind, SampleKind::Null));
    }

    #[tokio::test]
    async fn run_pipeline_with_trace_records_steps_and_durations() -> Result<()> {
        use crate::models::pipeline::PipelineStep;
        let steps = vec![
            PipelineStep::Pick {
                path: "items".into(),
            },
            PipelineStep::Limit { count: 2 },
            PipelineStep::Length,
        ];
        let initial = json!({ "items": [{"id":1},{"id":2},{"id":3},{"id":4}] });
        let (final_value, traces) = run_pipeline_with_trace(initial, &steps, None, None).await;
        assert_eq!(final_value, json!(2));
        assert_eq!(traces.len(), 3);
        assert_eq!(traces[0].kind, "pick");
        assert_eq!(traces[1].kind, "limit");
        assert_eq!(traces[2].kind, "length");
        // Limit step output is an array head with 2 items.
        assert_eq!(traces[1].output_sample.size_hint.items, Some(2));
        for step in &traces {
            assert!(step.error.is_none());
        }
        Ok(())
    }

    #[tokio::test]
    async fn unsupported_runtime_nodes_fail_explicitly() -> Result<()> {
        let wf = workflow(
            vec![WorkflowNode {
                id: "llm".into(),
                kind: NodeKind::Llm,
                label: "LLM".into(),
                position: None,
                config: Some(json!({ "prompt": "summarize" })),
            }],
            vec![],
        );

        let execution = WorkflowEngine::new().execute(&wf, None).await?;

        assert!(matches!(execution.run.status, RunStatus::Error));
        assert!(execution
            .run
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("AI runtime is unavailable"));

        Ok(())
    }
}

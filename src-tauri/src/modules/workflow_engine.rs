use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use tracing::{error, info};

use crate::models::chat::{ChatMessage, ChatMode, MessageRole};
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
                mcp_manager
                    .call_tool(server_id, tool_name, config.get("arguments").cloned())
                    .await
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

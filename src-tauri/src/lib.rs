pub mod commands;
pub mod models;
pub mod modules;

use serde_json::json;
use tauri::{App, Manager};
use tracing::info;

// ─── Module Managers ─────────────────────────────────────────────────────────

use modules::ai::AIEngine;
use modules::mcp_manager::MCPManager;
use modules::memory::MemoryEngine;
use modules::scheduler::Scheduler;
use modules::storage::Storage;
use modules::tool_engine::ToolEngine;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Global application state
#[derive(Clone)]
pub struct AppState {
    pub storage: Arc<Storage>,
    pub mcp_manager: Arc<MCPManager>,
    pub scheduler: Arc<Mutex<Scheduler>>,
    pub tool_engine: Arc<ToolEngine>,
    pub ai_engine: Arc<AIEngine>,
    pub memory_engine: Arc<MemoryEngine>,
    pub chat_abort_flags: Arc<dashmap::DashMap<String, Arc<AtomicBool>>>,
    /// W18: widget_id -> pending reflection registration. Populated by
    /// `apply_build_proposal_inner` for every new/replaced widget that
    /// originated from a chat session; consumed by `refresh_widget` once
    /// the widget renders real data for the first time.
    pub pending_reflections: Arc<dashmap::DashMap<String, ReflectionPending>>,
}

/// W18: one entry per widget waiting for its first successful refresh.
#[derive(Clone, Debug)]
pub struct ReflectionPending {
    pub session_id: String,
    pub dashboard_id: String,
    pub widget_id: String,
    pub widget_title: String,
    pub widget_kind: &'static str,
    pub replaced: bool,
    pub applied_at: i64,
}

impl AppState {
    pub async fn new(app: &App) -> anyhow::Result<Self> {
        let storage = Storage::new(app).await?;
        storage.migrate().await?;

        let storage = Arc::new(storage);
        let mcp_manager = Arc::new(MCPManager::new());
        let mut scheduler = Scheduler::new();
        scheduler.start().await?;
        let tool_engine = Arc::new(ToolEngine::default());
        let ai_engine = Arc::new(AIEngine::default());
        let memory_engine = Arc::new(MemoryEngine::new(storage.clone()));
        let app_handle = app.handle().clone();
        let provider = active_provider_for_startup(storage.as_ref()).await?;
        for workflow in storage
            .list_workflows()
            .await?
            .into_iter()
            .filter(|workflow| workflow.is_enabled)
        {
            let raw_cron = workflow
                .trigger
                .config
                .as_ref()
                .and_then(|config| config.cron.as_deref())
                .filter(|cron| !cron.trim().is_empty())
                .map(ToString::to_string);
            if let Some(raw_cron) = raw_cron {
                let Some(cron) = commands::dashboard::normalize_cron_expression(&raw_cron) else {
                    tracing::warn!(
                        "skipping startup scheduling for workflow '{}': cron '{}' is not parseable",
                        workflow.id,
                        raw_cron
                    );
                    continue;
                };
                let runtime = modules::scheduler::ScheduledRuntime {
                    app: app_handle.clone(),
                    storage: storage.clone(),
                    tool_engine: tool_engine.clone(),
                    mcp_manager: mcp_manager.clone(),
                    ai_engine: ai_engine.clone(),
                    provider: provider.clone(),
                };
                scheduler.schedule_cron(workflow, &cron, runtime).await?;
            }
        }
        let scheduler = Arc::new(Mutex::new(scheduler));

        Ok(Self {
            storage,
            mcp_manager,
            scheduler,
            tool_engine,
            ai_engine,
            memory_engine,
            chat_abort_flags: Arc::new(dashmap::DashMap::new()),
            pending_reflections: Arc::new(dashmap::DashMap::new()),
        })
    }
}

async fn active_provider_for_startup(
    storage: &Storage,
) -> anyhow::Result<Option<models::provider::LLMProvider>> {
    let providers = storage.list_providers().await?;
    let active_provider_id = storage
        .get_config("active_provider_id")
        .await?
        .filter(|id| !id.trim().is_empty());
    Ok(active_provider_id
        .as_deref()
        .and_then(|id| {
            providers
                .iter()
                .find(|provider| provider.id == id && provider.is_enabled)
        })
        .or_else(|| providers.iter().find(|provider| provider.is_enabled))
        .cloned())
}

// ─── Generate Tauri Command Handler ──────────────────────────────────────────

#[macro_export]
macro_rules! generate_handler {
    () => {
        tauri::generate_handler![
            // Dashboard commands
            $crate::commands::dashboard::list_dashboards,
            $crate::commands::dashboard::get_dashboard,
            $crate::commands::dashboard::create_dashboard,
            $crate::commands::dashboard::update_dashboard,
            $crate::commands::dashboard::add_dashboard_widget,
            $crate::commands::dashboard::apply_build_change,
            $crate::commands::dashboard::apply_build_proposal,
            $crate::commands::dashboard::dry_run_widget,
            $crate::commands::dashboard::delete_dashboard,
            $crate::commands::dashboard::refresh_widget,
            // W19: dashboard versions / undo
            $crate::commands::dashboard::list_dashboard_versions,
            $crate::commands::dashboard::get_dashboard_version,
            $crate::commands::dashboard::diff_dashboard_versions,
            $crate::commands::dashboard::restore_dashboard_version,
            // W25: dashboard parameters
            $crate::commands::dashboard::list_dashboard_parameters,
            $crate::commands::dashboard::get_dashboard_parameter_values,
            $crate::commands::dashboard::set_dashboard_parameter_value,
            $crate::commands::dashboard::resolve_dashboard_parameters,
            // Chat commands
            $crate::commands::chat::list_sessions,
            $crate::commands::chat::list_session_summaries,
            $crate::commands::chat::get_session,
            $crate::commands::chat::create_session,
            $crate::commands::chat::send_message,
            $crate::commands::chat::send_message_stream,
            $crate::commands::chat::cancel_chat_response,
            $crate::commands::chat::truncate_chat_messages,
            $crate::commands::chat::delete_session,
            // MCP commands
            $crate::commands::mcp::list_servers,
            $crate::commands::mcp::add_server,
            $crate::commands::mcp::remove_server,
            $crate::commands::mcp::enable_server,
            $crate::commands::mcp::reconnect_enabled_servers,
            $crate::commands::mcp::disable_server,
            $crate::commands::mcp::list_tools,
            $crate::commands::mcp::call_tool,
            // Provider commands
            $crate::commands::provider::list_providers,
            $crate::commands::provider::add_provider,
            $crate::commands::provider::update_provider,
            $crate::commands::provider::set_provider_enabled,
            $crate::commands::provider::remove_provider,
            $crate::commands::provider::test_provider,
            // Workflow commands
            $crate::commands::workflow::list_workflows,
            $crate::commands::workflow::get_workflow,
            $crate::commands::workflow::execute_workflow,
            $crate::commands::workflow::create_workflow,
            $crate::commands::workflow::delete_workflow,
            // Tool commands
            $crate::commands::tool::get_whitelist,
            $crate::commands::tool::execute_curl,
            $crate::commands::tool::execute_http_request,
            // Memory commands (W17)
            $crate::commands::memory::list_memories,
            $crate::commands::memory::delete_memory,
            $crate::commands::memory::remember_memory,
            $crate::commands::memory::recall_memories,
            $crate::commands::memory::list_tool_shapes,
            $crate::commands::memory::list_memory_kinds,
            // Playground commands (W20)
            $crate::commands::playground::list_playground_presets,
            $crate::commands::playground::save_playground_preset,
            $crate::commands::playground::delete_playground_preset,
            // Alert commands (W21)
            $crate::commands::alert::list_alert_events,
            $crate::commands::alert::acknowledge_alert,
            $crate::commands::alert::get_widget_alerts,
            $crate::commands::alert::set_widget_alerts,
            $crate::commands::alert::count_unacknowledged_alerts,
            $crate::commands::alert::test_alert_condition,
            // Debug commands (W23)
            $crate::commands::debug::trace_widget_pipeline,
            $crate::commands::debug::list_widget_traces,
            $crate::commands::debug::get_widget_trace,
            $crate::commands::debug::set_widget_capture_traces,
            // Cost commands (W22)
            $crate::commands::cost::get_session_cost_snapshot,
            $crate::commands::cost::get_cost_summary,
            $crate::commands::cost::set_session_budget,
            $crate::commands::cost::get_pricing_overrides,
            $crate::commands::cost::set_pricing_overrides,
            // Config commands
            $crate::commands::config::get_config,
            $crate::commands::config::set_config,
            // System commands
            $crate::commands::system::get_app_info,
            $crate::commands::system::open_url,
        ]
    };
}

// ─── Initialization ──────────────────────────────────────────────────────────

pub async fn init_storage(app: &App) -> anyhow::Result<()> {
    let state = AppState::new(app).await?;
    info!("📦 Storage initialized");

    if let Some(report_path) = std::env::var_os("DATRINA_E2E_REPORT") {
        let report_path = PathBuf::from(report_path);
        let app_handle = app.handle().clone();
        let smoke_state = state.clone();
        app.manage(state);
        tauri::async_runtime::spawn(async move {
            let code = match run_startup_e2e_smoke(smoke_state, report_path).await {
                Ok(()) => 0,
                Err(error) => {
                    tracing::error!("Datrina startup e2e smoke failed: {error:?}");
                    1
                }
            };
            app_handle.exit(code);
        });
        return Ok(());
    }

    app.manage(state);
    Ok(())
}

async fn run_startup_e2e_smoke(state: AppState, report_path: PathBuf) -> anyhow::Result<()> {
    use crate::models::dashboard::Dashboard;
    use crate::models::workflow::RunStatus;
    use crate::modules::workflow_engine::WorkflowEngine;

    let now = chrono::Utc::now().timestamp_millis();
    let (layout, workflows) = commands::dashboard::local_mvp_slice(now);
    let dashboard = Dashboard {
        id: uuid::Uuid::new_v4().to_string(),
        name: "Autopilot E2E Dashboard".to_string(),
        description: Some("Created by DATRINA_E2E_REPORT startup smoke.".to_string()),
        layout,
        workflows,
        is_default: false,
        created_at: now,
        updated_at: now,
        parameters: Vec::new(),
    };

    for workflow in &dashboard.workflows {
        state.storage.create_workflow(workflow).await?;
    }
    state.storage.create_dashboard(&dashboard).await?;

    let workflow = dashboard
        .workflows
        .first()
        .ok_or_else(|| anyhow::anyhow!("local MVP dashboard has no workflow"))?;
    let widget = dashboard
        .layout
        .first()
        .ok_or_else(|| anyhow::anyhow!("local MVP dashboard has no widget"))?;

    let engine = WorkflowEngine::with_runtime(
        state.tool_engine.as_ref(),
        state.mcp_manager.as_ref(),
        state.ai_engine.as_ref(),
        active_provider_for_startup(state.storage.as_ref()).await?,
    );
    let execution = engine.execute(workflow, None).await?;
    let run = execution.run;
    state.storage.save_workflow_run(&workflow.id, &run).await?;
    state
        .storage
        .update_workflow_last_run(&workflow.id, &run)
        .await?;

    let success = matches!(run.status, RunStatus::Success);
    let output_value = run
        .node_results
        .as_ref()
        .and_then(|value| value.pointer("/output/value"))
        .cloned();

    let report = json!({
        "success": success && output_value == Some(json!(72)),
        "dashboard_id": dashboard.id,
        "widget_id": widget.id(),
        "workflow_id": workflow.id,
        "workflow_run_id": run.id,
        "workflow_status": run.status,
        "workflow_error": run.error,
        "output_value": output_value,
        "created_at": now
    });

    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;

    if report
        .get("success")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "startup e2e smoke did not produce value 72"
        ))
    }
}

pub fn init_mcp_manager(_app: &App) -> anyhow::Result<()> {
    // MCP servers will be loaded from config on first access
    info!("📡 MCP manager ready");
    Ok(())
}

pub fn init_scheduler(_app: &App) -> anyhow::Result<()> {
    info!("⏰ Scheduler ready");
    Ok(())
}

// Helper to access state from commands
pub fn state(app: &tauri::AppHandle) -> tauri::State<'_, AppState> {
    app.state::<AppState>()
}

pub mod commands;
pub mod models;
pub mod modules;

use tauri::{App, Manager};
use tracing::info;

// ─── Module Managers ─────────────────────────────────────────────────────────

use modules::ai::AIEngine;
use modules::mcp_manager::MCPManager;
use modules::scheduler::Scheduler;
use modules::storage::Storage;
use modules::tool_engine::ToolEngine;

use std::sync::Arc;
use tokio::sync::Mutex;

/// Global application state
pub struct AppState {
    pub storage: Arc<Storage>,
    pub mcp_manager: Arc<MCPManager>,
    pub scheduler: Arc<Mutex<Scheduler>>,
    pub tool_engine: Arc<ToolEngine>,
    pub ai_engine: Arc<AIEngine>,
}

impl AppState {
    pub async fn new(app: &App) -> anyhow::Result<Self> {
        let storage = Storage::new(app).await?;
        storage.migrate().await?;

        let storage = Arc::new(storage);
        let mcp_manager = Arc::new(MCPManager::new());
        let mut scheduler = Scheduler::new();
        scheduler.start().await?;
        let scheduler = Arc::new(Mutex::new(scheduler));
        let tool_engine = Arc::new(ToolEngine::default());
        let ai_engine = Arc::new(AIEngine::default());

        Ok(Self {
            storage,
            mcp_manager,
            scheduler,
            tool_engine,
            ai_engine,
        })
    }
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
            $crate::commands::dashboard::delete_dashboard,
            $crate::commands::dashboard::refresh_widget,
            // Chat commands
            $crate::commands::chat::list_sessions,
            $crate::commands::chat::get_session,
            $crate::commands::chat::create_session,
            $crate::commands::chat::send_message,
            $crate::commands::chat::delete_session,
            // MCP commands
            $crate::commands::mcp::list_servers,
            $crate::commands::mcp::add_server,
            $crate::commands::mcp::remove_server,
            $crate::commands::mcp::enable_server,
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

    app.manage(state);
    Ok(())
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

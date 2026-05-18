use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

use crate::models::provider::LLMProvider;
use crate::models::workflow::{Workflow, WORKFLOW_EVENT_CHANNEL};
use crate::models::Id;
use crate::modules::ai::AIEngine;
use crate::modules::mcp_manager::MCPManager;
use crate::modules::storage::Storage;
use crate::modules::tool_engine::ToolEngine;
use crate::modules::workflow_engine::WorkflowEngine;

/// Manages scheduled workflow execution
pub struct Scheduler {
    scheduler: Option<JobScheduler>,
    jobs: HashMap<Id, uuid::Uuid>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            scheduler: None,
            jobs: HashMap::new(),
        }
    }

    /// Start the scheduler
    pub async fn start(&mut self) -> Result<()> {
        let sched = JobScheduler::new().await?;
        sched.start().await?;
        self.scheduler = Some(sched);
        info!("⏰ Scheduler started");
        Ok(())
    }

    /// Schedule a workflow with cron expression and execute through the same
    /// persisted runner used by manual workflow commands.
    pub async fn schedule_cron(
        &mut self,
        workflow: Workflow,
        cron_expr: &str,
        runtime: ScheduledRuntime,
    ) -> Result<()> {
        self.unschedule(&workflow.id).await?;
        let sched = self
            .scheduler
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Scheduler not started"))?;

        let workflow_id = workflow.id.clone();
        let workflow_name = workflow.name.clone();
        let cron_owned = cron_expr.to_string();

        let job = Job::new_async(cron_expr, move |_uuid, _l| {
            let workflow = workflow.clone();
            let cron = cron_owned.clone();
            let runtime = runtime.clone();
            Box::pin(async move {
                info!("⏰ Cron matched workflow {} (cron: {})", workflow.id, cron);
                if let Err(error) = execute_scheduled_workflow(workflow, runtime).await {
                    error!("Scheduled workflow execution failed: {}", error);
                }
            })
        })?;

        let job_id = job.guid();
        sched.add(job).await?;

        self.jobs.insert(workflow_id, job_id);
        info!(
            "📅 Scheduled workflow '{}' with cron: {}",
            workflow_name, cron_expr
        );

        Ok(())
    }

    /// Unschedule a workflow
    pub async fn unschedule(&mut self, workflow_id: &str) -> Result<()> {
        if let Some(job_id) = self.jobs.remove(workflow_id) {
            if let Some(sched) = &self.scheduler {
                sched.remove(&job_id).await?;
                info!("📅 Unscheduled workflow {}", workflow_id);
            }
        }
        Ok(())
    }

    /// Stop the scheduler
    pub async fn stop(&mut self) -> Result<()> {
        // Unschedule all
        for (workflow_id, _) in self.jobs.drain() {
            info!("📅 Unscheduled workflow {}", workflow_id);
        }

        if let Some(mut sched) = self.scheduler.take() {
            sched.shutdown().await?;
        }

        info!("⏰ Scheduler stopped");
        Ok(())
    }

    /// Get all scheduled workflow IDs
    pub fn list_scheduled(&self) -> Vec<Id> {
        self.jobs.keys().cloned().collect()
    }
}

#[derive(Clone)]
pub struct ScheduledRuntime {
    pub app: AppHandle,
    pub storage: Arc<Storage>,
    pub tool_engine: Arc<ToolEngine>,
    pub mcp_manager: Arc<MCPManager>,
    pub ai_engine: Arc<AIEngine>,
    pub provider: Option<LLMProvider>,
}

async fn execute_scheduled_workflow(
    workflow: Workflow,
    runtime: ScheduledRuntime,
) -> anyhow::Result<()> {
    reconnect_enabled_mcp_servers(&runtime).await?;
    // W47: scheduled workflows run outside a session/dashboard context,
    // so the app default is the only relevant language policy. Errors
    // here are swallowed because a missing language is "auto", not a
    // failure mode.
    let language_directive =
        crate::commands::language::resolve_effective_language(runtime.storage.as_ref(), None, None)
            .await
            .ok()
            .and_then(|resolved| resolved.system_directive());
    let engine = WorkflowEngine::with_runtime(
        runtime.tool_engine.as_ref(),
        runtime.mcp_manager.as_ref(),
        runtime.ai_engine.as_ref(),
        active_provider(&runtime)
            .await?
            .or(runtime.provider.clone()),
    )
    .with_storage(runtime.storage.as_ref())
    .with_language(language_directive);
    let execution = engine.execute(&workflow, None).await?;
    let run = execution.run;
    runtime
        .storage
        .save_workflow_run(&workflow.id, &run)
        .await?;
    runtime
        .storage
        .update_workflow_last_run(&workflow.id, &run)
        .await?;
    for event in execution.events {
        runtime.app.emit(WORKFLOW_EVENT_CHANNEL, event)?;
    }
    Ok(())
}

async fn active_provider(runtime: &ScheduledRuntime) -> anyhow::Result<Option<LLMProvider>> {
    // W29: cron-driven workflows reuse the typed resolver. When the
    // operator hasn't picked / has disabled the active provider, we
    // fall through to the runtime's captured provider (set at schedule
    // time) so a transient settings change doesn't kill an in-flight
    // cron tick.
    match crate::resolve_active_provider(runtime.storage.as_ref()).await? {
        Ok(provider) => Ok(Some(provider)),
        Err(_setup_error) => Ok(None),
    }
}

async fn reconnect_enabled_mcp_servers(runtime: &ScheduledRuntime) -> anyhow::Result<()> {
    let servers = runtime.storage.list_mcp_servers().await?;
    for server in servers.into_iter().filter(|server| server.is_enabled) {
        if runtime.mcp_manager.is_connected(&server.id).await {
            continue;
        }
        runtime.tool_engine.validate_mcp_server(&server)?;
        runtime.mcp_manager.connect(server).await?;
    }
    Ok(())
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

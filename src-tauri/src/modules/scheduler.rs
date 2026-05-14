use anyhow::Result;
use std::collections::HashMap;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::info;

use crate::models::workflow::Workflow;
use crate::models::Id;

/// Manages scheduled workflow execution
pub struct Scheduler {
    scheduler: Option<JobScheduler>,
    jobs: HashMap<Id, uuid::Uuid>,
    registration_only: bool,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            scheduler: None,
            jobs: HashMap::new(),
            registration_only: true,
        }
    }

    /// Start the scheduler
    pub async fn start(&mut self) -> Result<()> {
        let sched = JobScheduler::new().await?;
        self.scheduler = Some(sched);
        info!("⏰ Scheduler started");
        Ok(())
    }

    /// Schedule a workflow with cron expression
    pub async fn schedule_cron(&mut self, workflow: &Workflow, cron_expr: &str) -> Result<()> {
        let sched = self
            .scheduler
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Scheduler not started"))?;

        let workflow_id = workflow.id.clone();
        let cron_owned = cron_expr.to_string();

        let job = Job::new_async(cron_expr, move |_uuid, _l| {
            let wid = workflow_id.clone();
            let cron = cron_owned.clone();
            Box::pin(async move {
                info!(
                    "⏰ Cron matched workflow {} (cron: {}), but scheduler is registration-only in MVP baseline",
                    wid, cron
                );
            })
        })?;

        let job_id = job.guid();
        sched.add(job).await?;

        self.jobs.insert(workflow.id.clone(), job_id);
        info!(
            "📅 Scheduled workflow '{}' with cron: {}",
            workflow.name, cron_expr
        );

        Ok(())
    }

    /// MVP scheduler scope: cron registrations are tracked, but execution is
    /// intentionally not triggered until the workflow runner is injected here.
    pub fn is_registration_only(&self) -> bool {
        self.registration_only
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

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

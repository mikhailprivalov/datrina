use serde::{Deserialize, Serialize};

/// W16: a single structured issue raised by the proposal validator. The
/// agent (on retry) and the UI both consume these. New variants must be
/// mirrored in `src/lib/api.ts` for the typed frontend contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ValidationIssue {
    /// Widget has neither a `datasource_plan` nor a `source_key` linking
    /// to a `shared_datasources` entry. Without one, refresh cannot wire
    /// a workflow.
    MissingDatasourcePlan {
        widget_index: u32,
        widget_title: String,
    },
    /// Widget's `replace_widget_id` does not match any persisted widget
    /// on the target dashboard.
    UnknownReplaceWidgetId {
        widget_index: u32,
        widget_title: String,
        replace_widget_id: String,
    },
    /// `datasource_plan.source_key` references a key that is not declared
    /// in `proposal.shared_datasources`.
    UnknownSourceKey {
        widget_index: u32,
        widget_title: String,
        source_key: String,
    },
    /// Stat / gauge / bar_gauge has a numeric literal baked into config
    /// instead of resolving from the pipeline output. Heuristic — flagged
    /// when `config.value` is a non-null literal AND no pipeline produces
    /// the same path.
    HardcodedLiteralValue {
        widget_index: u32,
        widget_title: String,
        path: String,
    },
    /// Text widget body parses as JSON. The user has been explicit that
    /// text widgets must be markdown summaries, never raw JSON dumps.
    TextWidgetContainsRawJson {
        widget_index: u32,
        widget_title: String,
    },
    /// Widget kind requires a successful `dry_run_widget` tool call in
    /// the current chat session before the final proposal. None found.
    MissingDryRunEvidence {
        widget_index: u32,
        widget_title: String,
        widget_kind: String,
    },
    /// Pipeline step array failed strict typed deserialisation. Will
    /// reject apply later, surface now.
    PipelineSchemaInvalid {
        widget_index: u32,
        widget_title: String,
        error: String,
    },
    /// `shared_datasources` key collision: two entries share the same key.
    DuplicateSharedKey { key: String },
}

impl ValidationIssue {
    /// One-line summary suitable for embedding in the synthetic retry
    /// feedback the validator hands back to the agent.
    pub fn summary(&self) -> String {
        match self {
            ValidationIssue::MissingDatasourcePlan {
                widget_index,
                widget_title,
            } => format!(
                "widget #{widget_index} '{widget_title}' has no datasource_plan and no shared source_key — it cannot fetch data."
            ),
            ValidationIssue::UnknownReplaceWidgetId {
                widget_index,
                widget_title,
                replace_widget_id,
            } => format!(
                "widget #{widget_index} '{widget_title}' replaces widget id '{replace_widget_id}', but no such widget exists on the dashboard."
            ),
            ValidationIssue::UnknownSourceKey {
                widget_index,
                widget_title,
                source_key,
            } => format!(
                "widget #{widget_index} '{widget_title}' references shared source_key '{source_key}', which is not declared in proposal.shared_datasources."
            ),
            ValidationIssue::HardcodedLiteralValue {
                widget_index,
                widget_title,
                path,
            } => format!(
                "widget #{widget_index} '{widget_title}' has a hardcoded literal at '{path}'. Values must come from the datasource pipeline, not be embedded in config."
            ),
            ValidationIssue::TextWidgetContainsRawJson {
                widget_index,
                widget_title,
            } => format!(
                "text widget #{widget_index} '{widget_title}' contains raw JSON. Text widgets must be markdown summarising the data, not JSON dumps."
            ),
            ValidationIssue::MissingDryRunEvidence {
                widget_index,
                widget_title,
                widget_kind,
            } => format!(
                "widget #{widget_index} '{widget_title}' (kind={widget_kind}) requires a successful dry_run_widget call before the final proposal. Run dry_run_widget and confirm the data path works."
            ),
            ValidationIssue::PipelineSchemaInvalid {
                widget_index,
                widget_title,
                error,
            } => format!(
                "widget #{widget_index} '{widget_title}' has an invalid pipeline: {error}."
            ),
            ValidationIssue::DuplicateSharedKey { key } => {
                format!("shared_datasources contains duplicate key '{key}'.")
            }
        }
    }
}

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
    /// W25: widget references `$param_name` but no matching parameter is
    /// declared on the proposal or the existing dashboard.
    UnknownParameterReference {
        widget_index: u32,
        widget_title: String,
        param_name: String,
    },
    /// W25: parameter `depends_on` graph contains a cycle.
    ParameterCycle { cycle: Vec<String> },
    /// W38: targeted Build turn — the user mentioned specific widgets,
    /// but the proposal replaces a widget id that was not in the
    /// mention target set.
    OffTargetWidgetReplace {
        widget_index: u32,
        widget_title: String,
        replace_widget_id: String,
    },
    /// W38: targeted Build turn — the proposal removes a widget id that
    /// was not in the mention target set.
    OffTargetWidgetRemove { remove_widget_id: String },
    /// W39: a widget (or shared) datasource with `kind=builtin_tool,
    /// tool_name=http_request` failed the structural / safety gate
    /// (bad method, blocked URL, credential header, missing url, ...).
    /// `source_kind` is `"widget"` or `"shared"`; for widget sources
    /// the index/title locate it in the proposal, for shared sources
    /// `widget_title` carries the shared key.
    UnsafeHttpDatasource {
        widget_index: u32,
        widget_title: String,
        source_kind: String,
        reason: String,
    },
    /// W44: gallery widget bakes a hardcoded array of image items in
    /// `data` without a pipeline producing them. Gallery items must
    /// come from the datasource pipeline so refreshes change content.
    HardcodedGalleryItems {
        widget_index: u32,
        widget_title: String,
        item_count: u32,
    },
    /// W45: agent proposed explicit `x`/`y` for a new widget. Auto-pack
    /// always wins on the 12-col grid; the validator surfaces this so
    /// the next retry drops the coordinate guess instead of relying on
    /// silent fallback.
    ProposedExplicitCoordinates {
        widget_index: u32,
        widget_title: String,
    },
    /// W45: widget declares both a `size_preset` and explicit `w`/`h`.
    /// The two paths produce different sizes; force the agent to pick
    /// one — preferring the preset.
    ConflictingLayoutFields {
        widget_index: u32,
        widget_title: String,
    },
    /// W48: the user named one or more data sources via `@source` and
    /// the resulting proposal does not reference them. Each entry in
    /// `missing` is the stable identity of an unused mention (datasource
    /// definition id when available, workflow id otherwise, plus the
    /// display label so the retry feedback reads naturally).
    UnusedSourceMention {
        missing: Vec<UnusedSourceMentionEntry>,
    },
}

/// W48: companion struct for [`ValidationIssue::UnusedSourceMention`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnusedSourceMentionEntry {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datasource_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
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
            ValidationIssue::UnknownParameterReference {
                widget_index,
                widget_title,
                param_name,
            } => format!(
                "widget #{widget_index} '{widget_title}' references parameter '${param_name}', but no such parameter is declared in proposal.parameters or on the dashboard."
            ),
            ValidationIssue::ParameterCycle { cycle } => format!(
                "dashboard parameters form a depends_on cycle: {}.",
                cycle.join(" → ")
            ),
            ValidationIssue::OffTargetWidgetReplace {
                widget_index,
                widget_title,
                replace_widget_id,
            } => format!(
                "widget #{widget_index} '{widget_title}' replaces widget id '{replace_widget_id}', which the user did NOT mention this turn. Targeted edits must scope replacements to mentioned widgets only."
            ),
            ValidationIssue::OffTargetWidgetRemove { remove_widget_id } => format!(
                "remove_widget_ids includes '{remove_widget_id}', which the user did NOT mention this turn. Targeted edits must only remove mentioned widgets unless the user asked for broader cleanup."
            ),
            ValidationIssue::UnsafeHttpDatasource {
                widget_index,
                widget_title,
                source_kind,
                reason,
            } => format!(
                "{source_kind} datasource for widget #{widget_index} '{widget_title}' was rejected: {reason}. Fix the http_request arguments before re-proposing."
            ),
            ValidationIssue::HardcodedGalleryItems {
                widget_index,
                widget_title,
                item_count,
            } => format!(
                "gallery widget #{widget_index} '{widget_title}' embeds {item_count} hardcoded image items in `data`. Gallery items must come from the datasource pipeline output, not a baked array."
            ),
            ValidationIssue::ProposedExplicitCoordinates {
                widget_index,
                widget_title,
            } => format!(
                "widget #{widget_index} '{widget_title}' sets explicit `x`/`y`. The grid auto-packs new widgets row-first on 12 columns — drop `x`/`y` and order widgets in the array instead."
            ),
            ValidationIssue::ConflictingLayoutFields {
                widget_index,
                widget_title,
            } => format!(
                "widget #{widget_index} '{widget_title}' sets both `size_preset` and explicit `w`/`h`. Pick one — prefer `size_preset` (kpi / half_width / wide_chart / full_width / table / text_panel / gallery)."
            ),
            ValidationIssue::UnusedSourceMention { missing } => {
                let names = missing
                    .iter()
                    .map(|entry| {
                        let id = entry
                            .datasource_definition_id
                            .clone()
                            .or_else(|| entry.workflow_id.clone())
                            .unwrap_or_default();
                        if id.is_empty() {
                            entry.label.clone()
                        } else {
                            format!("{} ({})", entry.label, id)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "Build proposal does not reference these mentioned source(s): {names}. Every @source mentioned this turn must appear in the resulting widget's datasource_plan (use kind='compose' to combine multiple sources, or bind a single widget to one of them)."
                )
            }
        }
    }
}

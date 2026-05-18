use super::{Id, Timestamp};
use crate::models::widget::Widget;
use crate::models::workflow::Workflow;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dashboard {
    pub id: Id,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub layout: Vec<Widget>,
    pub workflows: Vec<Workflow>,
    pub is_default: bool,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// W25: Grafana-style template variables. Empty by default; references
    /// like `$name` / `${name}` inside widget configs are substituted by
    /// the parameter engine before workflow execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<DashboardParameter>,
    /// W43: optional default LLM policy for LLM-backed widgets on this
    /// dashboard. Overridable per widget via
    /// [`crate::models::widget::DatasourceConfig::model_override`]. When
    /// `None`, eligible widgets resolve to the app-level active provider
    /// only (no dashboard-level override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_policy: Option<DashboardModelPolicy>,
    /// W47: optional dashboard-level assistant language policy. When set,
    /// it overrides the app default for chat sessions scoped to this
    /// dashboard and for LLM-backed pipeline steps on this dashboard's
    /// widgets. `None` falls back to the app default (then to `Auto`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language_policy: Option<crate::models::language::AssistantLanguagePolicy>,
}

/// W43: dashboard-level default for LLM-backed widget runs. Credentials
/// are intentionally absent — only `provider_id` is stored; the api key
/// is loaded from the matching provider row at runtime. `required_caps`
/// is empty by default; callers may pin a capability (e.g. structured
/// JSON output for a Chart widget that asks the LLM to emit data points)
/// and the runtime fails closed with a typed error when the resolved
/// model cannot satisfy it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DashboardModelPolicy {
    pub provider_id: Id,
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_caps: Vec<crate::models::provider::WidgetCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateDashboardRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<CreateDashboardTemplate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateDashboardTemplate {
    Blank,
    LocalMvp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDashboardRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<Vec<Widget>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflows: Option<Vec<Workflow>>,
}

/// W43: dedicated request for the dashboard-level default model. Kept
/// off `UpdateDashboardRequest` so the caller can clear the policy by
/// sending `policy: null` without having to fetch and round-trip the
/// whole dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDashboardModelPolicyRequest {
    pub dashboard_id: Id,
    /// `None` clears the dashboard default and falls back to the app
    /// active provider for eligible widgets.
    pub policy: Option<DashboardModelPolicy>,
}

/// W47: dedicated request for the dashboard-level assistant language
/// policy. `policy: None` clears the override and falls back to the app
/// default (then to `Auto`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDashboardLanguagePolicyRequest {
    pub dashboard_id: Id,
    pub policy: Option<crate::models::language::AssistantLanguagePolicy>,
}

/// W43: dedicated request for a single widget's LLM override. `policy:
/// None` clears the override, falling back to the dashboard default
/// (and then to the app active provider).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetWidgetModelOverrideRequest {
    pub dashboard_id: Id,
    pub widget_id: Id,
    pub policy: Option<crate::models::widget::WidgetModelOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddWidgetRequest {
    pub widget_type: DashboardWidgetType,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardWidgetType {
    Text,
    Gauge,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyBuildChangeRequest {
    pub action: BuildChangeAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildChangeAction {
    CreateLocalDashboard,
    AddTextWidget,
    AddGaugeWidget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyBuildProposalRequest {
    pub proposal: BuildProposal,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    pub confirmed: bool,
    /// W18: chat session that produced this proposal. When set, the apply
    /// path registers a post-apply reflection job for each newly created
    /// or replaced widget so the agent can critique its own output after
    /// the first successful refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildProposal {
    pub id: Id,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dashboard_description: Option<String>,
    #[serde(default)]
    pub widgets: Vec<BuildWidgetProposal>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_widget_ids: Vec<Id>,
    /// Named upstream datasources shared by several widgets. Each entry runs
    /// its source + base pipeline once per refresh; widgets that reference
    /// the entry by `source_key` get fanned out from the same data with
    /// their own per-widget pipeline tail. Saves MCP/HTTP calls and keeps
    /// related widgets consistent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_datasources: Vec<SharedDatasource>,
    /// W25: dashboard-level template variables proposed alongside the
    /// widgets. Existing parameters with matching `id` are replaced; new
    /// ones are appended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<DashboardParameter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedDatasource {
    /// Stable name used by consumer widgets via
    /// `datasource_plan.source_key`. Must be unique within this proposal.
    pub key: String,
    pub kind: BuildDatasourcePlanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Optional base pipeline applied once to the raw source output before
    /// it is fanned out to consumer widgets. Each consumer can apply its
    /// own pipeline on top.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<crate::models::pipeline::PipelineStep>,
    /// Optional cron expression for periodic refresh. The cron is attached
    /// to the shared workflow, so a single tick refreshes every consumer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
    /// Optional human label for tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildWidgetProposal {
    pub widget_type: BuildWidgetType,
    pub title: String,
    #[serde(default)]
    pub data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datasource_plan: Option<BuildDatasourcePlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub w: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_widget_id: Option<Id>,
    /// W45: named size preset. When set, the apply path resolves the
    /// preset against the widget kind to derive (w, h); any explicit
    /// `w`/`h` on this widget is treated as a conflict and rejected by
    /// the validator. Apply also ignores `x`/`y` for new widgets per the
    /// 12-col auto-pack invariant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_preset: Option<SizePreset>,
    /// W45: layout pattern hint at the widget level. The apply path uses
    /// the pattern only for grouping / preset fallback; row breaks come
    /// from widget order and the 12-col packer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_pattern: Option<LayoutPattern>,
}

/// W45: named layout pattern the agent chose for this widget. The apply
/// path treats it as a soft size hint (when `size_preset` is missing)
/// and as a grouping signal — it does NOT translate to explicit
/// coordinates, since auto-pack still wins on the 12-col grid.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutPattern {
    /// 3–6 stat widgets across the top row.
    KpiRow,
    /// One wide chart spanning most of the row.
    TrendChartRow,
    /// Full-width operations table.
    OperationsTable,
    /// Datasource health / overview cards.
    DatasourceOverview,
    /// Image gallery / media board.
    MediaBoard,
    /// Markdown text panel + supporting metrics.
    TextPanel,
}

/// W45: discrete size preset. Resolved against the widget kind at apply
/// time to produce a (w, h) on the 12-col grid. Preferred over raw
/// `w`/`h` because it survives copy-paste between widget kinds and
/// stays predictable across refactors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SizePreset {
    /// KPI card. w 3, h 2 (4 per row).
    Kpi,
    /// Half-width panel. w 6.
    HalfWidth,
    /// Wide chart / bar_gauge. w 8.
    WideChart,
    /// Full-width chart or heatmap. w 12, modest height.
    FullWidth,
    /// Full-width table or logs. w 12, tall.
    Table,
    /// Text / markdown panel. w 6, h 4.
    TextPanel,
    /// Gallery / image. w 8, h 6.
    Gallery,
}

impl SizePreset {
    /// Resolve to (w, h) on the 12-col grid given the widget kind. The
    /// mapping is intentionally a small, complete table — adding a new
    /// widget kind requires extending this match.
    pub fn resolve(self, kind: &BuildWidgetType) -> (i32, i32) {
        use BuildWidgetType as K;
        use SizePreset as P;
        match (self, kind) {
            (P::Kpi, K::Stat) => (3, 2),
            (P::Kpi, K::Gauge) => (3, 3),
            (P::Kpi, K::BarGauge) => (4, 3),
            (P::Kpi, _) => (3, 2),

            (P::HalfWidth, K::Chart) => (6, 5),
            (P::HalfWidth, K::Table) => (6, 6),
            (P::HalfWidth, K::Logs) => (6, 6),
            (P::HalfWidth, K::BarGauge) => (6, 5),
            (P::HalfWidth, K::StatusGrid) => (6, 4),
            (P::HalfWidth, K::Text) => (6, 4),
            (P::HalfWidth, K::Heatmap) => (6, 6),
            (P::HalfWidth, K::Image) => (6, 4),
            (P::HalfWidth, K::Gallery) => (6, 5),
            (P::HalfWidth, _) => (6, 4),

            (P::WideChart, K::Chart) => (8, 5),
            (P::WideChart, K::BarGauge) => (8, 5),
            (P::WideChart, K::Heatmap) => (8, 6),
            (P::WideChart, K::StatusGrid) => (8, 5),
            (P::WideChart, _) => (8, 5),

            (P::FullWidth, K::Chart) => (12, 6),
            (P::FullWidth, K::Heatmap) => (12, 6),
            (P::FullWidth, K::Logs) => (12, 6),
            (P::FullWidth, K::BarGauge) => (12, 5),
            (P::FullWidth, K::Gallery) => (12, 6),
            (P::FullWidth, _) => (12, 5),

            (P::Table, K::Table) => (12, 8),
            (P::Table, K::Logs) => (12, 7),
            (P::Table, _) => (12, 6),

            (P::TextPanel, K::Text) => (6, 4),
            (P::TextPanel, _) => (6, 4),

            (P::Gallery, K::Gallery) => (8, 6),
            (P::Gallery, K::Image) => (8, 6),
            (P::Gallery, _) => (8, 6),
        }
    }
}

impl BuildWidgetType {
    /// Canonical lower-snake-case name. Used by validation messages and
    /// the Build prompt so widget kind strings stay consistent across
    /// Rust, TypeScript, and prompt text.
    pub fn label(&self) -> &'static str {
        match self {
            BuildWidgetType::Chart => "chart",
            BuildWidgetType::Text => "text",
            BuildWidgetType::Table => "table",
            BuildWidgetType::Image => "image",
            BuildWidgetType::Gauge => "gauge",
            BuildWidgetType::Stat => "stat",
            BuildWidgetType::Logs => "logs",
            BuildWidgetType::BarGauge => "bar_gauge",
            BuildWidgetType::StatusGrid => "status_grid",
            BuildWidgetType::Heatmap => "heatmap",
            BuildWidgetType::Gallery => "gallery",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildWidgetType {
    Chart,
    Text,
    Table,
    Image,
    Gauge,
    Stat,
    Logs,
    BarGauge,
    StatusGrid,
    Heatmap,
    /// W44: datasource-backed image gallery widget.
    Gallery,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildDatasourcePlan {
    pub kind: BuildDatasourcePlanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_cron: Option<String>,
    /// Optional deterministic transform pipeline applied to the datasource
    /// output before reaching the widget. Each step is a typed JSON object
    /// (see `models::pipeline::PipelineStep`); the final step may be an
    /// optional `llm_postprocess` for shapes the deterministic steps cannot
    /// produce on their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pipeline: Vec<crate::models::pipeline::PipelineStep>,
    /// For `kind: "shared"` plans, the `key` of the matching
    /// `proposal.shared_datasources` entry whose output feeds this widget.
    /// Other plan fields (tool_name, server_id, arguments, prompt,
    /// refresh_cron) are ignored when source_key is set; only `pipeline`
    /// (applied AFTER the shared base pipeline) is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    /// For `kind: "compose"` plans, the named inputs that should be fetched
    /// and exposed to the widget's `pipeline` as a single object
    /// `{ name1: <input1 output>, name2: <input2 output>, ... }`. Each
    /// inner plan is a regular `BuildDatasourcePlan` (mcp_tool, http_tool /
    /// builtin_tool, provider_prompt, or shared). Nested compose is not
    /// supported and is rejected at workflow build time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<std::collections::BTreeMap<String, BuildDatasourcePlan>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildDatasourcePlanKind {
    BuiltinTool,
    McpTool,
    /// Reference a `proposal.shared_datasources[<source_key>]` entry. The
    /// shared workflow handles the actual fetch + base pipeline; the
    /// widget's own `pipeline` field is applied on top as a per-widget
    /// tail.
    Shared,
    ProviderPrompt,
    /// Combine N independent inner plans into one widget. Each inner plan
    /// is fetched + shaped independently, then the results are merged into
    /// a single object keyed by `inputs` name, and the outer `pipeline` /
    /// `output_path` operate on that combined value. Lets a single widget
    /// pull from multiple sources (weather + air quality, releases +
    /// incidents, etc.) without fan-out workflows.
    Compose,
}

// ─── W19: Dashboard versions / undo ─────────────────────────────────────────

/// What caused a snapshot to be written. Drives the source badge in the
/// UI and lets the reflection heuristic correlate restores with their
/// parent versions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionSource {
    AgentApply,
    ManualEdit,
    Restore,
    PreDelete,
}

impl VersionSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            VersionSource::AgentApply => "agent_apply",
            VersionSource::ManualEdit => "manual_edit",
            VersionSource::Restore => "restore",
            VersionSource::PreDelete => "pre_delete",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "agent_apply" => Some(VersionSource::AgentApply),
            "manual_edit" => Some(VersionSource::ManualEdit),
            "restore" => Some(VersionSource::Restore),
            "pre_delete" => Some(VersionSource::PreDelete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardVersionSummary {
    pub id: Id,
    pub dashboard_id: Id,
    pub applied_at: Timestamp,
    pub source: VersionSource,
    pub summary: String,
    pub widget_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_version_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardVersion {
    pub id: Id,
    pub dashboard_id: Id,
    pub applied_at: Timestamp,
    pub source: VersionSource,
    pub summary: String,
    pub widget_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_version_id: Option<Id>,
    /// Full Dashboard serialized as JSON at snapshot time. Kept as a string
    /// in storage; deserialized on demand so callers that only need
    /// summaries do not pay the parse cost.
    pub snapshot: Dashboard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonPathChange {
    pub path: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetDiff {
    pub widget_id: Id,
    pub widget_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind_changed: Option<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_changed: Option<(String, String)>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config_changes: Vec<JsonPathChange>,
    pub datasource_plan_changed: bool,
    /// W31: `true` when the datasource identity changed — either the
    /// bound `datasource_definition_id`, the backing `workflow_id`, or
    /// `output_key`. Separated from `datasource_plan_changed` so the UI
    /// can highlight rebindings independently of per-widget tail edits.
    #[serde(default)]
    pub binding_changed: bool,
    /// W31: `true` when only the per-widget tail (post_process,
    /// capture_traces, binding_source/bound_at) changed without the
    /// identity moving.
    #[serde(default)]
    pub tail_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardDiff {
    pub from_version_id: Id,
    pub to_version_id: Id,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_widgets: Vec<WidgetSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_widgets: Vec<WidgetSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_widgets: Vec<WidgetDiff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name_changed: Option<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_changed: Option<(Option<String>, Option<String>)>,
    pub layout_changed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetSummary {
    pub id: Id,
    pub title: String,
    pub kind: String,
}

// ─── W25: Dashboard parameters (Grafana-style variables) ────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardParameter {
    pub id: String,
    pub name: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: DashboardParameterKind,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub include_all: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<ParameterValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DashboardParameterKind {
    StaticList {
        #[serde(default)]
        options: Vec<ParameterOption>,
    },
    TextInput {
        #[serde(skip_serializing_if = "Option::is_none")]
        placeholder: Option<String>,
    },
    TimeRange {
        #[serde(skip_serializing_if = "Option::is_none")]
        default_preset: Option<String>,
    },
    Interval {
        #[serde(default)]
        presets: Vec<String>,
    },
    McpQuery {
        server_id: String,
        tool_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        arguments: Option<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pipeline: Vec<crate::models::pipeline::PipelineStep>,
    },
    HttpQuery {
        method: String,
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        headers: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        body: Option<Value>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pipeline: Vec<crate::models::pipeline::PipelineStep>,
    },
    /// W34: pull options from a saved `DatasourceDefinition` and reshape
    /// its output with an optional parameter-specific tail pipeline that
    /// must produce a list of `{ label, value }` (or scalars, which are
    /// auto-doubled into label+value).
    DatasourceQuery {
        datasource_id: Id,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pipeline: Vec<crate::models::pipeline::PipelineStep>,
    },
    Constant {
        value: ParameterValue,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParameterValue {
    Bool(bool),
    Number(f64),
    String(String),
    Range(TimeRangeValue),
    Array(Vec<ParameterValue>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRangeValue {
    pub from: i64,
    pub to: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterOption {
    pub label: String,
    pub value: ParameterValue,
}

/// Persisted per-dashboard parameter selection. Stored separately from the
/// dashboard JSON so changing a value doesn't require rewriting the whole
/// layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardParameterSelection {
    pub dashboard_id: Id,
    pub param_name: String,
    pub value: ParameterValue,
    pub updated_at: Timestamp,
}

use super::Id;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    Chart {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: ChartConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_interval: Option<u32>,
    },
    Text {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: TextConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    Table {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: TableConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    Image {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: ImageConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    Gauge {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: GaugeConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    Stat {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: StatConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    Logs {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: LogsConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    BarGauge {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: BarGaugeConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    StatusGrid {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: StatusGridConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
    Heatmap {
        id: Id,
        title: String,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        config: HeatmapConfig,
        #[serde(skip_serializing_if = "Option::is_none")]
        datasource: Option<DatasourceConfig>,
    },
}

impl Widget {
    pub fn id(&self) -> &str {
        match self {
            Widget::Chart { id, .. } => id,
            Widget::Text { id, .. } => id,
            Widget::Table { id, .. } => id,
            Widget::Image { id, .. } => id,
            Widget::Gauge { id, .. } => id,
            Widget::Stat { id, .. } => id,
            Widget::Logs { id, .. } => id,
            Widget::BarGauge { id, .. } => id,
            Widget::StatusGrid { id, .. } => id,
            Widget::Heatmap { id, .. } => id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Widget::Chart { title, .. } => title,
            Widget::Text { title, .. } => title,
            Widget::Table { title, .. } => title,
            Widget::Image { title, .. } => title,
            Widget::Gauge { title, .. } => title,
            Widget::Stat { title, .. } => title,
            Widget::Logs { title, .. } => title,
            Widget::BarGauge { title, .. } => title,
            Widget::StatusGrid { title, .. } => title,
            Widget::Heatmap { title, .. } => title,
        }
    }
}

// ─── Widget Configs ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartConfig {
    pub kind: ChartKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_axis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_axis: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<String>>,
    #[serde(default)]
    pub stacked: bool,
    #[serde(default = "default_true")]
    pub show_legend: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    Line,
    Bar,
    Area,
    Pie,
    Scatter,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextConfig {
    #[serde(default = "default_text_format")]
    pub format: TextFormat,
    #[serde(default = "default_font_size")]
    pub font_size: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default = "default_align")]
    pub align: TextAlign,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFormat {
    Markdown,
    Plain,
    Html,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableConfig {
    pub columns: Vec<TableColumn>,
    #[serde(default = "default_page_size")]
    pub page_size: u16,
    #[serde(default = "default_true")]
    pub sortable: bool,
    #[serde(default)]
    pub filterable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableColumn {
    pub key: String,
    pub header: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
    #[serde(default = "default_column_format")]
    pub format: ColumnFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Vec<GaugeThreshold>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_colors: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_template: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnFormat {
    Text,
    Number,
    Date,
    Currency,
    Percent,
    Status,
    Progress,
    Badge,
    Link,
    Sparkline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageConfig {
    #[serde(default = "default_image_fit")]
    pub fit: ImageFit,
    #[serde(default = "default_border_radius")]
    pub border_radius: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFit {
    Cover,
    Contain,
    Fill,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaugeConfig {
    pub min: f64,
    pub max: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Vec<GaugeThreshold>>,
    #[serde(default = "default_true")]
    pub show_value: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GaugeThreshold {
    pub value: f64,
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suffix: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u8>,
    #[serde(default = "default_stat_color_mode")]
    pub color_mode: StatColorMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Vec<GaugeThreshold>>,
    #[serde(default = "default_true")]
    pub show_sparkline: bool,
    #[serde(default)]
    pub graph_mode: StatGraphMode,
    #[serde(default)]
    pub align: TextAlign,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatColorMode {
    #[default]
    None,
    Value,
    Background,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatGraphMode {
    #[default]
    None,
    Sparkline,
}

fn default_stat_color_mode() -> StatColorMode {
    StatColorMode::Value
}

// ─── Logs ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsConfig {
    #[serde(default = "default_max_entries")]
    pub max_entries: u32,
    #[serde(default = "default_true")]
    pub show_timestamp: bool,
    #[serde(default = "default_true")]
    pub show_level: bool,
    #[serde(default)]
    pub wrap: bool,
    #[serde(default)]
    pub reverse: bool,
}

fn default_max_entries() -> u32 {
    200
}

// ─── Bar Gauge ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarGaugeConfig {
    #[serde(default)]
    pub orientation: BarGaugeOrientation,
    #[serde(default)]
    pub display_mode: BarGaugeDisplayMode,
    #[serde(default = "default_true")]
    pub show_value: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thresholds: Option<Vec<GaugeThreshold>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarGaugeOrientation {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarGaugeDisplayMode {
    #[default]
    Gradient,
    Basic,
    Retro,
}

// ─── Status Grid ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusGridConfig {
    #[serde(default = "default_status_columns")]
    pub columns: u8,
    #[serde(default)]
    pub layout: StatusGridLayout,
    #[serde(default = "default_true")]
    pub show_label: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_colors: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusGridLayout {
    #[default]
    Grid,
    Row,
    Compact,
}

fn default_status_columns() -> u8 {
    4
}

// ─── Heatmap ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapConfig {
    #[serde(default = "default_heatmap_scheme")]
    pub color_scheme: HeatmapColorScheme,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default = "default_true")]
    pub show_legend: bool,
    #[serde(default)]
    pub log_scale: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeatmapColorScheme {
    #[default]
    Viridis,
    Magma,
    Cool,
    Warm,
    GreenRed,
}

fn default_heatmap_scheme() -> HeatmapColorScheme {
    HeatmapColorScheme::Viridis
}

// ─── Datasource ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceConfig {
    pub workflow_id: Id,
    pub output_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_process: Option<Vec<PostProcessStep>>,
    /// W23: opt-in pipeline trace capture on every refresh. The Debug
    /// view auto-enables this on first open so the next refresh becomes
    /// visible without manual intervention.
    #[serde(default, skip_serializing_if = "is_false")]
    pub capture_traces: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessStep {
    pub kind: PostProcessKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostProcessKind {
    LlmAnalyze,
    Filter,
    Aggregate,
    Transform,
    Sort,
    Limit,
}

// ─── Defaults ────────────────────────────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_text_format() -> TextFormat {
    TextFormat::Markdown
}

fn default_font_size() -> u8 {
    14
}

fn default_align() -> TextAlign {
    TextAlign::Left
}

fn default_page_size() -> u16 {
    10
}

fn default_column_format() -> ColumnFormat {
    ColumnFormat::Text
}

fn default_image_fit() -> ImageFit {
    ImageFit::Contain
}

fn default_border_radius() -> u8 {
    4
}

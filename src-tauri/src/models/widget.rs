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
}

impl Widget {
    pub fn id(&self) -> &str {
        match self {
            Widget::Chart { id, .. } => id,
            Widget::Text { id, .. } => id,
            Widget::Table { id, .. } => id,
            Widget::Image { id, .. } => id,
            Widget::Gauge { id, .. } => id,
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Widget::Chart { title, .. } => title,
            Widget::Text { title, .. } => title,
            Widget::Table { title, .. } => title,
            Widget::Image { title, .. } => title,
            Widget::Gauge { title, .. } => title,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnFormat {
    Text,
    Number,
    Date,
    Currency,
    Percent,
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

// ─── Datasource ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasourceConfig {
    pub workflow_id: Id,
    pub output_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_process: Option<Vec<PostProcessStep>>,
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

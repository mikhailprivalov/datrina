//! Deterministic data pipeline used by widget datasources.
//!
//! Each [`PipelineStep`] is a pure function over `serde_json::Value` (with
//! one explicit exception, [`PipelineStep::LlmPostprocess`], which calls the
//! active provider). The pipeline is described as data and serialized as
//! JSON, which lets the build chat agent compose strict deterministic
//! transforms instead of generating ad-hoc scripts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PipelineStep {
    /// Navigate to a sub-value using a dotted path with support for `[index]`,
    /// `[*]` to flatten an array, and bare numeric segments.
    Pick { path: String },
    /// Filter an array, keeping items where `field op value` evaluates to
    /// truthy. Non-array inputs become an empty array.
    Filter {
        field: String,
        #[serde(default)]
        op: FilterOp,
        #[serde(default)]
        value: serde_json::Value,
    },
    /// Sort an array by `by`. Non-array inputs pass through unchanged.
    Sort {
        by: String,
        #[serde(default)]
        order: SortOrder,
    },
    /// Keep the first `count` items of an array.
    Limit { count: usize },
    /// Reshape each item of an array: keep only `fields` (optionally renaming
    /// them via `rename`).
    Map {
        #[serde(default)]
        fields: Vec<String>,
        #[serde(default)]
        rename: std::collections::BTreeMap<String, String>,
    },
    /// Aggregate an array into a single object (or a list of group buckets).
    Aggregate {
        #[serde(default)]
        group_by: Option<String>,
        metric: AggregateMetric,
        #[serde(default = "default_output_key")]
        output_key: String,
    },
    /// Set or override a top-level field with a literal value.
    Set {
        field: String,
        value: serde_json::Value,
    },
    /// Take the first element of an array. Non-array inputs pass through.
    Head,
    /// Take the last element of an array.
    Tail,
    /// Replace the input with the length of the array (or 0).
    Length,
    /// Flatten one level of array-of-arrays.
    Flatten,
    /// Deduplicate array items by full equality (or by a field when given).
    Unique {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        by: Option<String>,
    },
    /// Render a string template with `{field}` placeholders pulled from the
    /// current object (top-level scope). For arrays, applies per item and
    /// returns an array of strings.
    Format {
        template: String,
        /// Optional output key. If set on a scalar input, the result is
        /// wrapped as `{ output_key: <formatted> }`. Defaults to replacing
        /// the input value with the formatted string.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_key: Option<String>,
    },
    /// Coerce the current value to a number or string. Useful right before
    /// a stat/gauge widget to ensure the runtime gets the right shape.
    Coerce { to: CoerceTarget },
    /// Optional LLM postprocess. Only invoked when the pipeline cannot
    /// produce the desired shape deterministically. The model receives the
    /// current pipeline output as JSON plus the prompt, and is asked to
    /// return JSON matching `expect`.
    LlmPostprocess {
        prompt: String,
        #[serde(default)]
        expect: LlmExpect,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    #[default]
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Contains,
    StartsWith,
    EndsWith,
    In,
    NotIn,
    Exists,
    NotExists,
    Truthy,
    Falsy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AggregateMetric {
    Count,
    Sum { field: String },
    Avg { field: String },
    Min { field: String },
    Max { field: String },
    First { field: String },
    Last { field: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmExpect {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoerceTarget {
    Number,
    String,
    Integer,
    Array,
}

fn default_output_key() -> String {
    "value".to_string()
}

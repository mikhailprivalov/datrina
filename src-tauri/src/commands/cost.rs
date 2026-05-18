use serde::{Deserialize, Serialize};
use tauri::State;

use crate::models::chat::{ChatSession, CostSessionEntry};
use crate::models::pricing::{ModelPricingOverride, PricingOverridesFile};
use crate::models::ApiResult;
use crate::AppState;

/// W22: snapshot returned to the chat footer. Combines live session
/// running totals with today's global spend so the UI can render
/// "12.4k in / 8.2k out · $0.043 · today $0.84" without making three
/// separate IPC calls per render. W49: `cost_unknown_turns` is the
/// count of assistant turns whose pricing was missing — when > 0 the
/// UI renders the total as a lower bound (`≥ $X.XXXX`) or as
/// `unknown cost` when `cost_usd` is still 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCostSnapshot {
    pub session_id: String,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
    pub max_cost_usd: Option<f64>,
    pub today_cost_usd: f64,
    #[serde(default)]
    pub cost_unknown_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_cost_source: Option<crate::models::pricing::CostSource>,
}

/// Daily roll-up entry for the Costs view bar chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyCostBucket {
    /// UTC midnight epoch millis for the day this bucket covers.
    pub day_start_ms: i64,
    pub cost_usd: f64,
}

/// Combined response for `get_cost_summary`. Single round trip for the
/// Costs view page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostSummary {
    pub today_cost_usd: f64,
    pub last_30_days: Vec<DailyCostBucket>,
    pub top_sessions: Vec<CostSessionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSessionBudgetRequest {
    pub session_id: String,
    pub max_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetPricingOverridesRequest {
    pub overrides: Vec<ModelPricingOverride>,
}

const ONE_DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// W22: live footer snapshot. Re-computes today's total each call —
/// the SQL roll-up is a single `SUM` against an index-friendly column
/// so this stays cheap even with thousands of sessions.
#[tauri::command]
pub async fn get_session_cost_snapshot(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<ApiResult<SessionCostSnapshot>, String> {
    let session = match state.storage.get_chat_session(&session_id).await {
        Ok(Some(session)) => session,
        Ok(None) => return Ok(ApiResult::err("session not found".to_string())),
        Err(error) => return Ok(ApiResult::err(error.to_string())),
    };
    let today_start = today_start_ms();
    let today = state
        .storage
        .sum_cost_between(today_start, today_start + ONE_DAY_MS)
        .await
        .unwrap_or(0.0);
    Ok(ApiResult::ok(SessionCostSnapshot {
        session_id: session.id.clone(),
        model: latest_assistant_model(&session),
        input_tokens: session.total_input_tokens,
        output_tokens: session.total_output_tokens,
        reasoning_tokens: session.total_reasoning_tokens,
        cost_usd: session.total_cost_usd,
        max_cost_usd: session.max_cost_usd,
        today_cost_usd: today,
        cost_unknown_turns: session.cost_unknown_turns,
        latest_cost_source: latest_assistant_cost_source(&session),
    }))
}

/// Costs view aggregate. 30-day window, top-5 sessions, today's total.
#[tauri::command]
pub async fn get_cost_summary(
    state: State<'_, AppState>,
    days: Option<u32>,
) -> Result<ApiResult<CostSummary>, String> {
    let day_count = days.unwrap_or(30).clamp(1, 365) as i64;
    let today_start = today_start_ms();
    let window_start = today_start - (day_count - 1) * ONE_DAY_MS;
    let buckets = match state
        .storage
        .daily_cost_buckets(window_start, today_start + ONE_DAY_MS)
        .await
    {
        Ok(b) => b,
        Err(error) => return Ok(ApiResult::err(error.to_string())),
    };
    let top_sessions = match state.storage.top_sessions_by_cost(5).await {
        Ok(s) => s,
        Err(error) => return Ok(ApiResult::err(error.to_string())),
    };
    let today = state
        .storage
        .sum_cost_between(today_start, today_start + ONE_DAY_MS)
        .await
        .unwrap_or(0.0);
    let last_30_days = buckets
        .into_iter()
        .map(|(day_start_ms, cost_usd)| DailyCostBucket {
            day_start_ms,
            cost_usd,
        })
        .collect();
    Ok(ApiResult::ok(CostSummary {
        today_cost_usd: today,
        last_30_days,
        top_sessions,
    }))
}

/// Set or clear the session-scoped USD budget cap.
#[tauri::command]
pub async fn set_session_budget(
    state: State<'_, AppState>,
    req: SetSessionBudgetRequest,
) -> Result<ApiResult<ChatSession>, String> {
    if let Some(max) = req.max_cost_usd {
        if !(max.is_finite()) || max < 0.0 {
            return Ok(ApiResult::err(
                "max_cost_usd must be a non-negative finite number".to_string(),
            ));
        }
    }
    match state
        .storage
        .set_session_max_cost(&req.session_id, req.max_cost_usd)
        .await
    {
        Ok(Some(session)) => Ok(ApiResult::ok(session)),
        Ok(None) => Ok(ApiResult::err("session not found".to_string())),
        Err(error) => Ok(ApiResult::err(error.to_string())),
    }
}

#[tauri::command]
pub async fn get_pricing_overrides(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<ModelPricingOverride>>, String> {
    Ok(
        match crate::commands::chat::load_pricing_overrides(state.inner()).await {
            Ok(overrides) => ApiResult::ok(overrides),
            Err(error) => ApiResult::err(error.to_string()),
        },
    )
}

#[tauri::command]
pub async fn set_pricing_overrides(
    state: State<'_, AppState>,
    req: SetPricingOverridesRequest,
) -> Result<ApiResult<Vec<ModelPricingOverride>>, String> {
    let path = state.storage.pricing_overrides_path();
    let payload = PricingOverridesFile {
        overrides: req.overrides.clone(),
    };
    let serialised = match serde_json::to_vec_pretty(&payload) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Ok(ApiResult::err(format!(
                "could not serialise pricing overrides: {error}"
            )));
        }
    };
    if let Some(parent) = path.parent() {
        if let Err(error) = tokio::fs::create_dir_all(parent).await {
            return Ok(ApiResult::err(error.to_string()));
        }
    }
    if let Err(error) = tokio::fs::write(&path, serialised).await {
        return Ok(ApiResult::err(error.to_string()));
    }
    Ok(ApiResult::ok(req.overrides))
}

fn latest_assistant_model(session: &ChatSession) -> Option<String> {
    session.messages.iter().rev().find_map(|message| {
        message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.model.clone())
    })
}

/// W49: most recent assistant turn's pricing provenance, so the footer
/// can render a "billed by provider" vs "local pricing table" hint.
fn latest_assistant_cost_source(
    session: &ChatSession,
) -> Option<crate::models::pricing::CostSource> {
    session.messages.iter().rev().find_map(|message| {
        message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.cost_source)
    })
}

fn today_start_ms() -> i64 {
    let now = chrono::Utc::now();
    let truncated = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or(now.naive_utc())
        .and_utc();
    truncated.timestamp_millis()
}

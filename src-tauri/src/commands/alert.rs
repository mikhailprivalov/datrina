use tauri::State;

use crate::models::alert::{
    AlertEvent, SetWidgetAlertsRequest, TestAlertConditionRequest, TestAlertConditionResult,
    WidgetAlert,
};
use crate::models::ApiResult;
use crate::modules::alert_engine;
use crate::AppState;

#[tauri::command]
pub async fn list_alert_events(
    state: State<'_, AppState>,
    only_unacknowledged: Option<bool>,
    limit: Option<u32>,
) -> Result<ApiResult<Vec<AlertEvent>>, String> {
    let only_unack = only_unacknowledged.unwrap_or(false);
    let limit = limit.unwrap_or(200) as usize;
    Ok(
        match state.storage.list_alert_events(only_unack, limit).await {
            Ok(events) => ApiResult::ok(events),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn acknowledge_alert(
    state: State<'_, AppState>,
    event_id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(
        match state.storage.acknowledge_alert_event(&event_id).await {
            Ok(updated) => ApiResult::ok(updated),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn get_widget_alerts(
    state: State<'_, AppState>,
    widget_id: String,
) -> Result<ApiResult<Vec<WidgetAlert>>, String> {
    Ok(match state.storage.get_widget_alerts(&widget_id).await {
        Ok(alerts) => ApiResult::ok(alerts),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn set_widget_alerts(
    state: State<'_, AppState>,
    req: SetWidgetAlertsRequest,
) -> Result<ApiResult<Vec<WidgetAlert>>, String> {
    if let Err(e) = state
        .storage
        .set_widget_alerts(&req.widget_id, &req.dashboard_id, &req.alerts)
        .await
    {
        return Ok(ApiResult::err(e.to_string()));
    }
    Ok(ApiResult::ok(req.alerts))
}

#[tauri::command]
pub async fn count_unacknowledged_alerts(
    state: State<'_, AppState>,
) -> Result<ApiResult<i64>, String> {
    Ok(match state.storage.count_unacknowledged_alerts().await {
        Ok(n) => ApiResult::ok(n),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn test_alert_condition(
    req: TestAlertConditionRequest,
) -> Result<ApiResult<TestAlertConditionResult>, String> {
    let (fired, resolved_value, reason) = alert_engine::test_condition(&req.condition, &req.data);
    Ok(ApiResult::ok(TestAlertConditionResult {
        fired,
        resolved_value,
        reason,
    }))
}

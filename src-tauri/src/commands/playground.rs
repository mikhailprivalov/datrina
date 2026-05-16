use tauri::State;

use crate::models::playground::{PlaygroundPreset, SavePlaygroundPresetRequest};
use crate::models::ApiResult;
use crate::AppState;

#[tauri::command]
pub async fn list_playground_presets(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<PlaygroundPreset>>, String> {
    Ok(match state.storage.list_playground_presets().await {
        Ok(presets) => ApiResult::ok(presets),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn save_playground_preset(
    state: State<'_, AppState>,
    req: SavePlaygroundPresetRequest,
) -> Result<ApiResult<PlaygroundPreset>, String> {
    let display_name = req.display_name.trim().to_string();
    if display_name.is_empty() {
        return Ok(ApiResult::err("Preset name must not be empty".to_string()));
    }
    let tool_name = req.tool_name.trim().to_string();
    if tool_name.is_empty() {
        return Ok(ApiResult::err("tool_name must not be empty".to_string()));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let preset = PlaygroundPreset {
        id: uuid::Uuid::new_v4().to_string(),
        tool_kind: req.tool_kind,
        server_id: req.server_id.filter(|s| !s.trim().is_empty()),
        tool_name,
        display_name,
        arguments: req.arguments,
        created_at: now,
        updated_at: now,
    };

    Ok(
        match state.storage.upsert_playground_preset(&preset).await {
            Ok(()) => ApiResult::ok(preset),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn delete_playground_preset(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.delete_playground_preset(&id).await {
        Ok(removed) => ApiResult::ok(removed),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

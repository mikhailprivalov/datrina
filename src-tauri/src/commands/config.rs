use tauri::State;

use crate::models::ApiResult;
use crate::AppState;

#[tauri::command]
pub async fn get_config(
    state: State<'_, AppState>,
    key: String,
) -> Result<ApiResult<Option<String>>, String> {
    Ok(match state.storage.get_config(&key).await {
        Ok(value) => ApiResult::ok(value),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn set_config(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.set_config(&key, &value).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

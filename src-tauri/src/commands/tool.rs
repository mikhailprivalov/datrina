use tauri::State;

use crate::models::ApiResult;
use crate::AppState;

#[tauri::command]
pub async fn get_whitelist(state: State<'_, AppState>) -> Result<ApiResult<Vec<String>>, String> {
    Ok(ApiResult::ok(state.tool_engine.get_whitelist()))
}

#[tauri::command]
pub async fn execute_curl(
    state: State<'_, AppState>,
    args: Vec<String>,
) -> Result<ApiResult<serde_json::Value>, String> {
    Ok(match state.tool_engine.execute_curl(args).await {
        Ok(result) => ApiResult::ok(result),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[derive(serde::Deserialize)]
pub struct HttpRequestArgs {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub body: Option<serde_json::Value>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
}

#[tauri::command]
pub async fn execute_http_request(
    state: State<'_, AppState>,
    req: HttpRequestArgs,
) -> Result<ApiResult<serde_json::Value>, String> {
    Ok(
        match state
            .tool_engine
            .http_request(&req.method, &req.url, req.body, req.headers)
            .await
        {
            Ok(result) => ApiResult::ok(result),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

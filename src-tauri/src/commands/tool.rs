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

/// Read the current default User-Agent string applied to every outbound
/// `http_request` (chat tool calls + widget pipelines).
#[tauri::command]
pub async fn get_http_user_agent(state: State<'_, AppState>) -> Result<ApiResult<String>, String> {
    Ok(ApiResult::ok(state.tool_engine.user_agent()))
}

/// Update the default User-Agent. Persists into the `config` table and
/// hot-swaps it on the live `ToolEngine` so the next request picks it up
/// without restarting the app. Empty input resets to the canonical Datrina UA.
#[tauri::command]
pub async fn set_http_user_agent(
    state: State<'_, AppState>,
    user_agent: String,
) -> Result<ApiResult<String>, String> {
    state.tool_engine.set_user_agent(&user_agent);
    let resolved = state.tool_engine.user_agent();
    if let Err(e) = state.storage.set_config("http_user_agent", &resolved).await {
        return Ok(ApiResult::err(e.to_string()));
    }
    Ok(ApiResult::ok(resolved))
}

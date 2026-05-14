use serde_json::json;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::models::ApiResult;

#[tauri::command]
pub fn get_app_info() -> ApiResult<serde_json::Value> {
    ApiResult::ok(json!({
        "name": "Datrina The Lenswright",
        "version": "0.1.0",
        "description": "AI-powered local dashboard with MCP integration",
    }))
}

#[tauri::command]
pub async fn open_url(app: AppHandle, url: String) -> ApiResult<()> {
    match app.opener().open_url(url, None::<&str>) {
        Ok(_) => ApiResult::ok(()),
        Err(e) => ApiResult::err(e.to_string()),
    }
}

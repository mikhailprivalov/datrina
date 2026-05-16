use tauri::State;

use crate::models::memory::{
    MemoryHit, MemoryKind, MemoryRecord, RecallRequest, RememberRequest, Scope, ToolShape,
};
use crate::models::ApiResult;
use crate::AppState;

#[tauri::command]
pub async fn list_memories(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<MemoryRecord>>, String> {
    Ok(match state.memory_engine.list().await {
        Ok(records) => ApiResult::ok(records),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn delete_memory(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.memory_engine.forget(&id).await {
        Ok(removed) => ApiResult::ok(removed),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn remember_memory(
    state: State<'_, AppState>,
    req: RememberRequest,
) -> Result<ApiResult<MemoryRecord>, String> {
    Ok(match state.memory_engine.remember(req).await {
        Ok(record) => ApiResult::ok(record),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn recall_memories(
    state: State<'_, AppState>,
    req: RecallRequest,
) -> Result<ApiResult<Vec<MemoryHit>>, String> {
    let mut scopes: Vec<Scope> = Vec::new();
    if let Some(dashboard_id) = req.dashboard_id.as_ref().filter(|id| !id.trim().is_empty()) {
        scopes.push(Scope::Dashboard(dashboard_id.clone()));
    }
    for server_id in req.mcp_server_ids.iter().filter(|id| !id.trim().is_empty()) {
        scopes.push(Scope::McpServer(server_id.clone()));
    }
    if let Some(session_id) = req.session_id.as_ref().filter(|id| !id.trim().is_empty()) {
        scopes.push(Scope::Session(session_id.clone()));
    }
    scopes.push(Scope::Global);
    Ok(
        match state
            .memory_engine
            .retrieve(&req.query, &scopes, req.top_n.max(1))
            .await
        {
            Ok(hits) => ApiResult::ok(hits),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn list_tool_shapes(
    state: State<'_, AppState>,
    server_id: String,
) -> Result<ApiResult<Vec<ToolShape>>, String> {
    Ok(
        match state.memory_engine.list_tool_shapes(&server_id).await {
            Ok(shapes) => ApiResult::ok(shapes),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

// Convenience for the admin UI: surface the discrete kinds so the
// frontend doesn't have to hard-code them.
#[tauri::command]
pub fn list_memory_kinds() -> Result<ApiResult<Vec<&'static str>>, String> {
    let kinds = vec![
        MemoryKind::Fact.as_str(),
        MemoryKind::Preference.as_str(),
        MemoryKind::ToolShape.as_str(),
        MemoryKind::Lesson.as_str(),
    ];
    Ok(ApiResult::ok(kinds))
}

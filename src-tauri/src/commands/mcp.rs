use tauri::State;
use tracing::info;

use crate::models::mcp::{CallToolRequest, MCPServer, MCPTool};
use crate::models::ApiResult;
use crate::AppState;

fn mask_env(mut server: MCPServer) -> MCPServer {
    if let Some(env) = server.env.as_mut() {
        for value in env.values_mut() {
            *value = "********".to_string();
        }
    }
    server
}

#[tauri::command]
pub async fn list_servers(state: State<'_, AppState>) -> Result<ApiResult<Vec<MCPServer>>, String> {
    Ok(match state.storage.list_mcp_servers().await {
        Ok(servers) => ApiResult::ok(servers.into_iter().map(mask_env).collect()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn add_server(
    state: State<'_, AppState>,
    server: MCPServer,
) -> Result<ApiResult<bool>, String> {
    if let Err(e) = state.tool_engine.validate_mcp_server(&server) {
        return Ok(ApiResult::err(e.to_string()));
    }

    Ok(match state.storage.save_mcp_server(&server).await {
        Ok(()) => {
            info!("Added MCP server: {}", server.name);
            ApiResult::ok(true)
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn remove_server(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    let _ = state.mcp_manager.disconnect(&id).await;

    Ok(match state.storage.delete_mcp_server(&id).await {
        Ok(true) => ApiResult::ok(true),
        Ok(false) => ApiResult::err("Server not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn enable_server(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<Vec<MCPTool>>, String> {
    let server = match state.storage.list_mcp_servers().await {
        Ok(servers) => servers.into_iter().find(|s| s.id == id),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    match server {
        Some(s) => {
            if let Err(e) = state.tool_engine.validate_mcp_server(&s) {
                return Ok(ApiResult::err(e.to_string()));
            }

            match state.mcp_manager.connect(s).await {
                Ok(tools) => {
                    info!("MCP server '{}' connected with {} tools", id, tools.len());
                    Ok(ApiResult::ok(tools))
                }
                Err(e) => Ok(ApiResult::err(e.to_string())),
            }
        }
        None => Ok(ApiResult::err("Server not found".to_string())),
    }
}

#[tauri::command]
pub async fn disable_server(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.mcp_manager.disconnect(&id).await {
        Ok(()) => ApiResult::ok(true),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn list_tools(state: State<'_, AppState>) -> Result<ApiResult<Vec<MCPTool>>, String> {
    let tools = state.mcp_manager.list_tools().await;
    Ok(ApiResult::ok(tools))
}

#[tauri::command]
pub async fn call_tool(
    state: State<'_, AppState>,
    req: CallToolRequest,
) -> Result<ApiResult<serde_json::Value>, String> {
    if let Err(e) = state
        .tool_engine
        .validate_mcp_tool_call(&req.server_id, &req.tool_name)
    {
        return Ok(ApiResult::err(e.to_string()));
    }

    Ok(
        match state
            .mcp_manager
            .call_tool(&req.server_id, &req.tool_name, req.arguments)
            .await
        {
            Ok(result) => ApiResult::ok(result),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

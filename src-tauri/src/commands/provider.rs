use tauri::State;
use tracing::info;

use crate::models::provider::{CreateProviderRequest, LLMProvider, ProviderTestResult};
use crate::models::ApiResult;
use crate::AppState;

fn without_secret(mut provider: LLMProvider) -> LLMProvider {
    provider.api_key = None;
    provider
}

#[tauri::command]
pub async fn list_providers(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<LLMProvider>>, String> {
    Ok(match state.storage.list_providers().await {
        Ok(providers) => ApiResult::ok(providers.into_iter().map(without_secret).collect()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn add_provider(
    state: State<'_, AppState>,
    req: CreateProviderRequest,
) -> Result<ApiResult<LLMProvider>, String> {
    let provider = LLMProvider {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        kind: req.kind,
        base_url: req.base_url,
        api_key: req.api_key,
        default_model: req.default_model,
        models: req.models.unwrap_or_default(),
        is_enabled: true,
    };

    if let Err(e) = crate::modules::ai::validate_provider(&provider) {
        return Ok(ApiResult::err(e.to_string()));
    }

    Ok(match state.storage.save_provider(&provider).await {
        Ok(()) => {
            info!("🤖 Added provider: {}", provider.name);
            ApiResult::ok(without_secret(provider))
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn remove_provider(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<bool>, String> {
    Ok(match state.storage.delete_provider(&id).await {
        Ok(true) => {
            info!("🗑️  Removed provider: {}", id);
            ApiResult::ok(true)
        }
        Ok(false) => ApiResult::err("Provider not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn test_provider(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApiResult<ProviderTestResult>, String> {
    Ok(match state.storage.get_provider(&id).await {
        Ok(Some(provider)) => ApiResult::ok(state.ai_engine.test_provider(&provider).await),
        Ok(None) => ApiResult::err("Provider not found".to_string()),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

use tauri::State;
use tracing::info;

use crate::models::provider::{
    CreateProviderRequest, LLMProvider, ProviderTestResult, UpdateProviderRequest,
};
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
        is_unsupported: false,
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
pub async fn update_provider(
    state: State<'_, AppState>,
    id: String,
    req: UpdateProviderRequest,
) -> Result<ApiResult<LLMProvider>, String> {
    let mut provider = match state.storage.get_provider(&id).await {
        Ok(Some(provider)) => provider,
        Ok(None) => return Ok(ApiResult::err("Provider not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    if let Some(name) = req.name {
        provider.name = name;
    }
    if let Some(kind) = req.kind {
        provider.kind = kind;
    }
    if let Some(base_url) = req.base_url {
        provider.base_url = base_url;
    }
    if let Some(api_key) = req.api_key {
        provider.api_key = if api_key.trim().is_empty() {
            None
        } else {
            Some(api_key)
        };
    }
    if let Some(default_model) = req.default_model {
        provider.default_model = default_model;
    }
    if let Some(models) = req.models {
        provider.models = models;
    }
    if let Some(is_enabled) = req.is_enabled {
        provider.is_enabled = is_enabled;
    }

    // W29: editing a legacy `unsupported` row through the standard
    // update path re-stamps it as a real product provider as long as
    // the caller has switched it to a supported `kind` and it now
    // validates. If they tried to leave it on the legacy kind, the
    // typed update request can't even express that (the union doesn't
    // include `local_mock`), so the field flip is implicit.
    provider.is_unsupported = false;

    if let Err(e) = crate::modules::ai::validate_provider(&provider) {
        return Ok(ApiResult::err(e.to_string()));
    }

    Ok(match state.storage.save_provider(&provider).await {
        Ok(()) => {
            info!("🤖 Updated provider: {}", provider.name);
            ApiResult::ok(without_secret(provider))
        }
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn set_provider_enabled(
    state: State<'_, AppState>,
    id: String,
    is_enabled: bool,
) -> Result<ApiResult<LLMProvider>, String> {
    let mut provider = match state.storage.get_provider(&id).await {
        Ok(Some(provider)) => provider,
        Ok(None) => return Ok(ApiResult::err("Provider not found".to_string())),
        Err(e) => return Ok(ApiResult::err(e.to_string())),
    };

    if is_enabled && provider.is_unsupported {
        return Ok(ApiResult::err(
            "provider_unsupported: this provider uses a legacy kind that is no longer supported. Edit it to a supported kind (OpenRouter, Ollama, or Custom) before enabling.".to_string(),
        ));
    }
    provider.is_enabled = is_enabled;
    if is_enabled {
        if let Err(e) = crate::modules::ai::validate_provider(&provider) {
            return Ok(ApiResult::err(e.to_string()));
        }
    }

    Ok(match state.storage.save_provider(&provider).await {
        Ok(()) => ApiResult::ok(without_secret(provider)),
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

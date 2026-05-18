//! W47: assistant language policy commands.
//!
//! Exposes the curated language catalog and the app-level / dashboard /
//! session policy mutators. The shared resolver
//! ([`resolve_effective_language`]) is the only path callers should use
//! when injecting a directive into a provider system prompt — it walks
//! the override stack (session → dashboard → app) and returns the
//! provenance the UI displays alongside the chosen tag.

use anyhow::Result as AnyResult;
use tauri::State;

use crate::models::chat::ChatSession;
use crate::models::dashboard::{Dashboard, SetDashboardLanguagePolicyRequest};
use crate::models::language::{
    find_language, language_catalog, parse_policy, AssistantLanguageOption,
    AssistantLanguagePolicy, AssistantLanguageSource, EffectiveAssistantLanguage,
    APP_LANGUAGE_CONFIG_KEY,
};
use crate::models::{ApiResult, Id};
use crate::modules::storage::Storage;
use crate::AppState;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AssistantLanguageCatalog {
    pub options: Vec<AssistantLanguageOption>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolveAssistantLanguageRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Id>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetSessionLanguageRequest {
    pub session_id: Id,
    /// `None` clears the per-session override and falls back to the
    /// dashboard / app stack.
    pub policy: Option<AssistantLanguagePolicy>,
}

#[tauri::command]
pub async fn list_assistant_languages() -> Result<ApiResult<AssistantLanguageCatalog>, String> {
    Ok(ApiResult::ok(AssistantLanguageCatalog {
        options: language_catalog(),
    }))
}

#[tauri::command]
pub async fn get_app_assistant_language(
    state: State<'_, AppState>,
) -> Result<ApiResult<AssistantLanguagePolicy>, String> {
    Ok(match load_app_policy(state.storage.as_ref()).await {
        Ok(policy) => ApiResult::ok(policy),
        Err(e) => ApiResult::err(e.to_string()),
    })
}

#[tauri::command]
pub async fn set_app_assistant_language(
    state: State<'_, AppState>,
    policy: AssistantLanguagePolicy,
) -> Result<ApiResult<AssistantLanguagePolicy>, String> {
    Ok(
        match set_app_policy(state.storage.as_ref(), &policy).await {
            Ok(()) => ApiResult::ok(policy),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

#[tauri::command]
pub async fn set_dashboard_language_policy(
    state: State<'_, AppState>,
    req: SetDashboardLanguagePolicyRequest,
) -> Result<ApiResult<Dashboard>, String> {
    Ok(
        match set_dashboard_policy_inner(state.storage.as_ref(), req).await {
            Ok(dashboard) => ApiResult::ok(dashboard),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn set_dashboard_policy_inner(
    storage: &Storage,
    req: SetDashboardLanguagePolicyRequest,
) -> AnyResult<Dashboard> {
    let mut dashboard = storage
        .get_dashboard(&req.dashboard_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Dashboard not found: {}", req.dashboard_id))?;
    validate_policy(req.policy.as_ref())?;
    dashboard.language_policy = req.policy;
    dashboard.updated_at = chrono::Utc::now().timestamp_millis();
    storage.update_dashboard(&dashboard).await?;
    Ok(dashboard)
}

#[tauri::command]
pub async fn set_session_language_policy(
    state: State<'_, AppState>,
    req: SetSessionLanguageRequest,
) -> Result<ApiResult<ChatSession>, String> {
    Ok(
        match set_session_policy_inner(state.storage.as_ref(), req).await {
            Ok(session) => ApiResult::ok(session),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

async fn set_session_policy_inner(
    storage: &Storage,
    req: SetSessionLanguageRequest,
) -> AnyResult<ChatSession> {
    validate_policy(req.policy.as_ref())?;
    storage
        .set_session_language_override(&req.session_id, req.policy.as_ref())
        .await?
        .ok_or_else(|| anyhow::anyhow!("Chat session not found: {}", req.session_id))
}

#[tauri::command]
pub async fn resolve_assistant_language(
    state: State<'_, AppState>,
    req: ResolveAssistantLanguageRequest,
) -> Result<ApiResult<EffectiveAssistantLanguage>, String> {
    Ok(
        match resolve_effective_language(
            state.storage.as_ref(),
            req.dashboard_id.as_deref(),
            req.session_id.as_deref(),
        )
        .await
        {
            Ok(resolved) => ApiResult::ok(resolved),
            Err(e) => ApiResult::err(e.to_string()),
        },
    )
}

/// W47: resolve the effective assistant language for a request scope.
/// Override priority (highest to lowest): session → dashboard → app
/// default. Unknown tags fall through transparently so a stale config
/// row never blocks chat — the caller sees `Auto` as the safe default.
pub async fn resolve_effective_language(
    storage: &Storage,
    dashboard_id: Option<&str>,
    session_id: Option<&str>,
) -> AnyResult<EffectiveAssistantLanguage> {
    if let Some(id) = session_id {
        if let Some(session) = storage.get_chat_session(id).await? {
            if let Some(option) = policy_to_option(session.language_override.as_ref()) {
                return Ok(EffectiveAssistantLanguage {
                    source: AssistantLanguageSource::SessionOverride,
                    option: Some(option),
                });
            }
            if matches!(
                session.language_override,
                Some(AssistantLanguagePolicy::Auto)
            ) {
                return Ok(EffectiveAssistantLanguage::auto());
            }
        }
    }

    if let Some(id) = dashboard_id {
        if let Some(dashboard) = storage.get_dashboard(id).await? {
            if let Some(option) = policy_to_option(dashboard.language_policy.as_ref()) {
                return Ok(EffectiveAssistantLanguage {
                    source: AssistantLanguageSource::DashboardOverride,
                    option: Some(option),
                });
            }
            if matches!(
                dashboard.language_policy,
                Some(AssistantLanguagePolicy::Auto)
            ) {
                return Ok(EffectiveAssistantLanguage::auto());
            }
        }
    }

    let app_policy = load_app_policy(storage).await?;
    if let Some(option) = policy_to_option(Some(&app_policy)) {
        Ok(EffectiveAssistantLanguage {
            source: AssistantLanguageSource::AppDefault,
            option: Some(option),
        })
    } else {
        Ok(EffectiveAssistantLanguage::auto())
    }
}

fn policy_to_option(policy: Option<&AssistantLanguagePolicy>) -> Option<AssistantLanguageOption> {
    match policy? {
        AssistantLanguagePolicy::Auto => None,
        AssistantLanguagePolicy::Explicit { tag } => find_language(tag),
    }
}

async fn load_app_policy(storage: &Storage) -> AnyResult<AssistantLanguagePolicy> {
    let raw = storage.get_config(APP_LANGUAGE_CONFIG_KEY).await?;
    Ok(raw
        .filter(|s| !s.trim().is_empty())
        .map(|s| parse_policy(&s))
        .unwrap_or_default())
}

async fn set_app_policy(storage: &Storage, policy: &AssistantLanguagePolicy) -> AnyResult<()> {
    validate_policy(Some(policy))?;
    let payload = serde_json::to_string(policy)?;
    storage
        .set_config(APP_LANGUAGE_CONFIG_KEY, &payload)
        .await?;
    Ok(())
}

fn validate_policy(policy: Option<&AssistantLanguagePolicy>) -> AnyResult<()> {
    match policy {
        None | Some(AssistantLanguagePolicy::Auto) => Ok(()),
        Some(AssistantLanguagePolicy::Explicit { tag }) => {
            if find_language(tag).is_some() {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "unknown assistant language tag '{}': not in the curated catalog (see list_assistant_languages)",
                    tag
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_unknown_tag() {
        let err = validate_policy(Some(&AssistantLanguagePolicy::Explicit {
            tag: "klingon".to_string(),
        }))
        .unwrap_err();
        assert!(err.to_string().contains("klingon"));
    }

    #[test]
    fn validate_accepts_catalog_tag() {
        assert!(validate_policy(Some(&AssistantLanguagePolicy::Explicit {
            tag: "ru".to_string()
        }))
        .is_ok());
    }

    #[test]
    fn validate_accepts_auto_and_none() {
        assert!(validate_policy(None).is_ok());
        assert!(validate_policy(Some(&AssistantLanguagePolicy::Auto)).is_ok());
    }

    #[test]
    fn policy_to_option_maps_explicit_to_catalog_entry() {
        let option = policy_to_option(Some(&AssistantLanguagePolicy::Explicit {
            tag: "ja".to_string(),
        }))
        .expect("japanese in catalog");
        assert_eq!(option.tag, "ja");
        assert_eq!(option.prompt_name, "Japanese");
    }

    #[test]
    fn policy_to_option_returns_none_for_auto() {
        assert!(policy_to_option(Some(&AssistantLanguagePolicy::Auto)).is_none());
    }
}

//! W37: external open-source catalog commands.
//!
//! The catalog itself (the [`ExternalSource`] vector) is built into the
//! binary — this file owns user state (enable/disable, credential) and
//! the runtime execution path that issues HTTP requests through the
//! existing `ToolEngine`. Disabled, blocked, or credential-missing
//! sources fail closed with a typed error before any network I/O.

use anyhow::{anyhow, bail, Result as AnyResult};
use chrono::Utc;
use serde_json::Value;
use tauri::{AppHandle, State};
use tracing::info;

use crate::commands::datasource::{
    create_datasource_definition_from_external_source, list_datasources_originated_from,
};
use crate::models::datasource::DatasourceDefinition;
use crate::models::external_source::{
    ExternalSource, ExternalSourceAdapter, ExternalSourceCredentialPolicy,
    ExternalSourceImpactPreview, ExternalSourceReviewStatus, ExternalSourceState,
    ExternalSourceTestRequest, ExternalSourceTestResult, ExternalSourceToolDescriptor,
    ExternalSourceWithState, OriginatingDatasource, SaveExternalSourceRequest,
    SaveExternalSourceResult,
};
use crate::models::ApiResult;
use crate::modules::external_source_catalog;
use crate::modules::workflow_engine::execute_pipeline_for_external_source;
use crate::AppState;

/// Fetch persisted state for a source, defaulting to "disabled, no credential".
async fn load_state(state: &AppState, source_id: &str) -> AnyResult<ExternalSourceState> {
    let now = Utc::now().timestamp_millis();
    match state.storage.get_external_source_state(source_id).await? {
        Some((enabled, credential, updated_at)) => Ok(ExternalSourceState {
            source_id: source_id.to_string(),
            is_enabled: enabled,
            has_credential: credential.is_some(),
            updated_at,
        }),
        None => Ok(ExternalSourceState {
            source_id: source_id.to_string(),
            is_enabled: false,
            has_credential: false,
            updated_at: now,
        }),
    }
}

/// Internal helper: returns the raw credential when present. Never exposed
/// to React — only used by the request builder.
async fn load_credential(state: &AppState, source_id: &str) -> AnyResult<Option<String>> {
    Ok(state
        .storage
        .get_external_source_state(source_id)
        .await?
        .and_then(|(_, credential, _)| credential))
}

fn runnable_reason(source: &ExternalSource, state: &ExternalSourceState) -> Option<String> {
    // W37++: catalog rows that describe third-party MCP servers are
    // *informational only* — Datrina does not execute them. The user
    // installs them via MCP Settings. Don't ever surface them to chat
    // as callable tools, even when "enabled" in the catalog.
    if matches!(source.adapter, ExternalSourceAdapter::McpRecommended) {
        return Some("install via MCP Settings (informational catalog row)".to_string());
    }
    if !source.review_status.is_runnable() {
        return Some(match source.review_status {
            ExternalSourceReviewStatus::Blocked => "blocked by Datrina review".to_string(),
            ExternalSourceReviewStatus::NeedsReview => "awaiting Datrina review".to_string(),
            _ => "not runnable".to_string(),
        });
    }
    if !state.is_enabled {
        return Some("disabled".to_string());
    }
    if matches!(
        source.credential_policy,
        ExternalSourceCredentialPolicy::Required
    ) && !state.has_credential
    {
        return Some("missing credential".to_string());
    }
    None
}

fn merge_with_state(
    source: &ExternalSource,
    state: ExternalSourceState,
) -> ExternalSourceWithState {
    let blocked_reason = runnable_reason(source, &state);
    let is_runnable = blocked_reason.is_none();
    ExternalSourceWithState {
        source: source.clone(),
        state,
        is_runnable,
        blocked_reason,
    }
}

#[tauri::command]
pub async fn list_external_sources(
    state: State<'_, AppState>,
) -> Result<ApiResult<Vec<ExternalSourceWithState>>, String> {
    let mut result = Vec::with_capacity(external_source_catalog::catalog().len());
    for entry in external_source_catalog::catalog() {
        let user_state = match load_state(&state, &entry.id).await {
            Ok(value) => value,
            Err(error) => return Ok(ApiResult::err(error.to_string())),
        };
        result.push(merge_with_state(entry, user_state));
    }
    Ok(ApiResult::ok(result))
}

#[tauri::command]
pub async fn set_external_source_enabled(
    state: State<'_, AppState>,
    source_id: String,
    enabled: bool,
) -> Result<ApiResult<ExternalSourceWithState>, String> {
    let Some(source) = external_source_catalog::find(&source_id) else {
        return Ok(ApiResult::err(format!(
            "External source '{}' is not in the catalog",
            source_id
        )));
    };
    if enabled && !source.review_status.is_runnable() {
        return Ok(ApiResult::err(format!(
            "Source '{}' cannot be enabled: review status is {:?}",
            source_id, source.review_status
        )));
    }
    let now = Utc::now().timestamp_millis();
    if let Err(error) = state
        .storage
        .upsert_external_source_state(&source_id, enabled, None, now)
        .await
    {
        return Ok(ApiResult::err(error.to_string()));
    }
    info!(
        "external source '{}' is now {}",
        source_id,
        if enabled { "enabled" } else { "disabled" }
    );
    let merged = match load_state(&state, &source_id).await {
        Ok(s) => merge_with_state(source, s),
        Err(error) => return Ok(ApiResult::err(error.to_string())),
    };
    Ok(ApiResult::ok(merged))
}

#[tauri::command]
pub async fn set_external_source_credential(
    state: State<'_, AppState>,
    source_id: String,
    credential: Option<String>,
) -> Result<ApiResult<ExternalSourceWithState>, String> {
    let Some(source) = external_source_catalog::find(&source_id) else {
        return Ok(ApiResult::err(format!(
            "External source '{}' is not in the catalog",
            source_id
        )));
    };
    if matches!(
        source.credential_policy,
        ExternalSourceCredentialPolicy::None
    ) {
        return Ok(ApiResult::err(format!(
            "Source '{}' does not accept a credential",
            source_id
        )));
    }
    let now = Utc::now().timestamp_millis();
    match credential {
        Some(value) if !value.trim().is_empty() => {
            // Preserve current enablement; just write the credential.
            let current = match load_state(&state, &source_id).await {
                Ok(s) => s,
                Err(error) => return Ok(ApiResult::err(error.to_string())),
            };
            if let Err(error) = state
                .storage
                .upsert_external_source_state(
                    &source_id,
                    current.is_enabled,
                    Some(value.trim()),
                    now,
                )
                .await
            {
                return Ok(ApiResult::err(error.to_string()));
            }
        }
        _ => {
            if let Err(error) = state
                .storage
                .clear_external_source_credential(&source_id)
                .await
            {
                return Ok(ApiResult::err(error.to_string()));
            }
        }
    }
    let merged = match load_state(&state, &source_id).await {
        Ok(s) => merge_with_state(source, s),
        Err(error) => return Ok(ApiResult::err(error.to_string())),
    };
    Ok(ApiResult::ok(merged))
}

#[tauri::command]
pub async fn test_external_source(
    state: State<'_, AppState>,
    req: ExternalSourceTestRequest,
) -> Result<ApiResult<ExternalSourceTestResult>, String> {
    Ok(match test_external_source_inner(&state, req).await {
        Ok(result) => ApiResult::ok(result),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

async fn test_external_source_inner(
    state: &AppState,
    req: ExternalSourceTestRequest,
) -> AnyResult<ExternalSourceTestResult> {
    let Some(source) = external_source_catalog::find(&req.source_id) else {
        bail!("External source '{}' is not in the catalog", req.source_id);
    };
    if !source.review_status.is_runnable() {
        bail!(
            "Source '{}' is not runnable (review status {:?})",
            source.id,
            source.review_status
        );
    }
    let user_state = load_state(state, &source.id).await?;
    if matches!(
        source.credential_policy,
        ExternalSourceCredentialPolicy::Required
    ) && !user_state.has_credential
    {
        bail!(
            "Source '{}' requires a credential before it can be tested",
            source.id
        );
    }
    let credential = load_credential(state, &source.id).await?;
    execute_external_source_request(state, source, &req.arguments, credential.as_deref()).await
}

/// Public entry-point used by `commands::chat` to fold enabled sources
/// into the chat tool dispatcher.
pub async fn run_external_source_tool(
    state: &AppState,
    source_id: &str,
    arguments: &Value,
) -> AnyResult<Value> {
    let Some(source) = external_source_catalog::find(source_id) else {
        bail!("External source '{}' is not in the catalog", source_id);
    };
    let user_state = load_state(state, source_id).await?;
    if let Some(reason) = runnable_reason(source, &user_state) {
        bail!(
            "External source '{}' cannot run: {}",
            source.display_name,
            reason
        );
    }
    let credential = load_credential(state, source_id).await?;
    let result =
        execute_external_source_request(state, source, arguments, credential.as_deref()).await?;
    // Surface the shaped final value to the chat loop. The raw payload
    // is dropped to keep tool-result tokens predictable; the test UI
    // still has access to it through `test_external_source`.
    Ok(serde_json::json!({
        "source_id": source.id,
        "attribution": source.attribution,
        "value": result.final_value,
        "duration_ms": result.duration_ms,
        "effective_url": result.effective_url,
    }))
}

/// Return one [`ExternalSourceToolDescriptor`] per source that is
/// currently runnable for the active profile.
pub async fn list_runnable_external_sources(
    state: &AppState,
) -> AnyResult<Vec<(ExternalSource, ExternalSourceToolDescriptor)>> {
    let mut out = Vec::new();
    for entry in external_source_catalog::catalog() {
        let user_state = load_state(state, &entry.id).await?;
        if runnable_reason(entry, &user_state).is_some() {
            continue;
        }
        let descriptor = ExternalSourceToolDescriptor {
            source_id: entry.id.clone(),
            tool_name: entry.tool_name(),
            description: format!(
                "{} {}",
                entry.description,
                entry
                    .attribution
                    .as_deref()
                    .map(|a| format!("Attribution: {}", a))
                    .unwrap_or_default()
            )
            .trim()
            .to_string(),
            parameters_schema: entry.tool_parameters_schema(),
        };
        out.push((entry.clone(), descriptor));
    }
    Ok(out)
}

async fn execute_external_source_request(
    state: &AppState,
    source: &ExternalSource,
    arguments: &Value,
    credential: Option<&str>,
) -> AnyResult<ExternalSourceTestResult> {
    let started = std::time::Instant::now();

    // W37++: catalog rows describing recommended MCP servers are not
    // executed by Datrina. They surface install metadata for MCP
    // Settings; refuse to fall through to the HTTP path.
    if matches!(source.adapter, ExternalSourceAdapter::McpRecommended) {
        bail!(
            "Source '{}' is an MCP install recommendation, not a callable tool. Use MCP Settings to install it.",
            source.id
        );
    }

    // W37++: WebFetch adapter routes through the safe single-URL path
    // (robots obedience + size cap) and short-circuits the rest of the
    // HttpJson pipeline. The catalog parameter `url` is what the user
    // provides; optional `max_bytes` caps the body.
    if matches!(source.adapter, ExternalSourceAdapter::WebFetch) {
        let args_obj = arguments.as_object().cloned().unwrap_or_default();
        let url = args_obj
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Missing required argument 'url' for source '{}'", source.id))?
            .trim()
            .to_string();
        let max_bytes = args_obj
            .get("max_bytes")
            .and_then(Value::as_u64)
            .map(|v| v as usize);
        let response = state.tool_engine.web_fetch(&url, max_bytes).await?;
        let raw_body = response.get("body").cloned().unwrap_or(Value::Null);
        let final_value =
            execute_pipeline_for_external_source(state, &source.default_pipeline, raw_body.clone())
                .await?;
        let effective_url = response
            .get("url")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
            .unwrap_or_else(|| url.clone());
        let _ = credential; // WebFetch never uses BYOK.
        return Ok(ExternalSourceTestResult {
            source_id: source.id.clone(),
            duration_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
            raw_response: response,
            final_value,
            pipeline_steps: source.default_pipeline.len() as u32,
            effective_url,
        });
    }

    // Substitute {name} tokens in URL + query + headers.
    let (effective_url, headers, body) = build_http_call(source, arguments, credential)?;

    let response = state
        .tool_engine
        .http_request(&source.http.method, &effective_url, body, Some(headers))
        .await?;

    // tool_engine returns { status, body }. Pull body out for shaping;
    // a non-2xx status is surfaced as an error so chat doesn't silently
    // pipeline an HTML error page.
    let status = response.get("status").and_then(Value::as_u64).unwrap_or(0);
    let raw_body = response.get("body").cloned().unwrap_or(Value::Null);
    if !(200..400).contains(&status) {
        bail!(
            "Source '{}' returned HTTP {}: {}",
            source.display_name,
            status,
            preview_string(&raw_body, 400)
        );
    }

    let final_value =
        execute_pipeline_for_external_source(state, &source.default_pipeline, raw_body.clone())
            .await?;
    Ok(ExternalSourceTestResult {
        source_id: source.id.clone(),
        duration_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
        raw_response: raw_body,
        final_value,
        pipeline_steps: source.default_pipeline.len() as u32,
        effective_url,
    })
}

/// W37: same builder as [`build_http_call`] but with the credential
/// header forcibly omitted. Used by the "save as datasource" path so a
/// real API key never ends up baked into a workflow JSON row.
pub fn build_http_call_for_save(
    source: &ExternalSource,
    arguments: &Value,
) -> AnyResult<(String, Value, Option<Value>)> {
    build_http_call(source, arguments, None)
}

fn build_http_call(
    source: &ExternalSource,
    arguments: &Value,
    credential: Option<&str>,
) -> AnyResult<(String, Value, Option<Value>)> {
    let args_obj = arguments.as_object().cloned().unwrap_or_default();

    // Required-param check before any substitution so the error is clear.
    for param in &source.params {
        if param.required {
            let provided = args_obj
                .get(&param.name)
                .map(|v| !is_blank(v))
                .unwrap_or(false);
            if !provided {
                bail!(
                    "Missing required argument '{}' for source '{}'",
                    param.name,
                    source.id
                );
            }
        }
    }

    let base_url = substitute_tokens(&source.http.url, &args_obj)?;

    // Append query parameters via reqwest::Url::query_pairs_mut so
    // characters get encoded the same way reqwest will send them.
    // Empty values after substitution are skipped so optional params
    // don't produce `?foo=&bar=baz`.
    let mut parsed = reqwest::Url::parse(&base_url).map_err(|e| {
        anyhow!(
            "invalid URL '{}' for source '{}': {}",
            base_url,
            source.id,
            e
        )
    })?;
    {
        let mut pairs = parsed.query_pairs_mut();
        for (key, template) in &source.http.query {
            let value = substitute_tokens(template, &args_obj)?;
            if value.trim().is_empty() {
                continue;
            }
            pairs.append_pair(key, &value);
        }
    }
    let url = parsed.to_string();

    let mut headers_map = serde_json::Map::new();
    for (key, value) in &source.http.headers {
        headers_map.insert(key.clone(), Value::String(value.clone()));
    }
    if let (Some(header_name), Some(cred)) = (source.http.credential_header.as_deref(), credential)
    {
        let prefix = source.http.credential_prefix.as_deref().unwrap_or("");
        headers_map.insert(
            header_name.to_string(),
            Value::String(format!("{}{}", prefix, cred)),
        );
    }

    let body_value = if source.http.body_params.is_empty() {
        None
    } else {
        let mut body = serde_json::Map::new();
        for name in &source.http.body_params {
            if let Some(value) = args_obj.get(name).cloned() {
                body.insert(name.clone(), value);
            }
        }
        Some(Value::Object(body))
    };

    Ok((url, Value::Object(headers_map), body_value))
}

fn substitute_tokens(template: &str, args: &serde_json::Map<String, Value>) -> AnyResult<String> {
    let mut output = String::with_capacity(template.len());
    let mut chars = template.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch != '{' {
            output.push(ch);
            continue;
        }
        let mut name = String::new();
        let mut closed = false;
        while let Some((_, c)) = chars.next() {
            if c == '}' {
                closed = true;
                break;
            }
            name.push(c);
        }
        if !closed {
            bail!("Unterminated '{{' in template: {}", template);
        }
        if name.is_empty() {
            output.push('{');
            output.push('}');
            continue;
        }
        match args.get(&name) {
            Some(value) => output.push_str(&value_to_string(value)),
            None => {} // optional / unset → leave empty
        }
    }
    Ok(output)
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}

fn preview_string(value: &Value, limit: usize) -> String {
    let s = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
    if s.len() <= limit {
        s
    } else {
        format!("{}…(truncated)", &s[..limit])
    }
}

#[tauri::command]
pub async fn preview_external_source_impact(
    state: State<'_, AppState>,
    source_id: String,
) -> Result<ApiResult<ExternalSourceImpactPreview>, String> {
    Ok(match preview_impact_inner(&state, &source_id).await {
        Ok(value) => ApiResult::ok(value),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

async fn preview_impact_inner(
    state: &State<'_, AppState>,
    source_id: &str,
) -> AnyResult<ExternalSourceImpactPreview> {
    if external_source_catalog::find(source_id).is_none() {
        bail!("External source '{}' is not in the catalog", source_id);
    }
    let datasources = list_datasources_originated_from(state, source_id).await?;
    let has_credential = state
        .storage
        .get_external_source_state(source_id)
        .await?
        .and_then(|(_, credential, _)| credential)
        .is_some();
    Ok(ExternalSourceImpactPreview {
        source_id: source_id.to_string(),
        originating_datasources: datasources
            .into_iter()
            .map(|def| OriginatingDatasource {
                datasource_id: def.id,
                name: def.name,
                workflow_id: def.workflow_id,
            })
            .collect(),
        has_credential,
    })
}

#[tauri::command]
pub async fn save_external_source_as_datasource(
    app: AppHandle,
    state: State<'_, AppState>,
    req: SaveExternalSourceRequest,
) -> Result<ApiResult<SaveExternalSourceResult>, String> {
    Ok(match save_external_source_inner(&app, &state, req).await {
        Ok(result) => ApiResult::ok(result),
        Err(error) => ApiResult::err(error.to_string()),
    })
}

async fn save_external_source_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    req: SaveExternalSourceRequest,
) -> AnyResult<SaveExternalSourceResult> {
    let Some(source) = external_source_catalog::find(&req.source_id) else {
        bail!("External source '{}' is not in the catalog", req.source_id);
    };
    if !source.review_status.is_runnable() {
        bail!(
            "Source '{}' is not runnable (review status {:?}); cannot save",
            source.id,
            source.review_status
        );
    }
    // W37++: only the HttpJson adapter maps onto a Datrina workflow
    // node. WebFetch lives behind `tool_engine.web_fetch` and is not
    // available as a workflow tool today; McpRecommended is informational.
    if !matches!(source.adapter, ExternalSourceAdapter::HttpJson) {
        bail!(
            "Source '{}' cannot be saved as a datasource: its adapter ({:?}) is not exposed as a workflow tool. Use chat for one-off calls.",
            source.id,
            source.adapter
        );
    }
    if req.name.trim().is_empty() {
        bail!("Datasource name is required");
    }
    let def: DatasourceDefinition = create_datasource_definition_from_external_source(
        app,
        state,
        source,
        req.name.trim(),
        &req.arguments,
        req.refresh_cron.as_deref(),
    )
    .await?;
    Ok(SaveExternalSourceResult {
        source_id: source.id.clone(),
        datasource_id: def.id,
        workflow_id: def.workflow_id,
    })
}

/// Helper used by the chat tool spec builder to format the source list
/// shown to the LLM. Only runnable sources are visible.
pub fn describe_runnable_sources(
    specs: &[(ExternalSource, ExternalSourceToolDescriptor)],
) -> String {
    if specs.is_empty() {
        return String::new();
    }
    specs
        .iter()
        .map(|(source, desc)| {
            format!(
                "- `{}` ({}): {}{}",
                desc.tool_name,
                source.display_name,
                desc.description,
                source
                    .attribution
                    .as_deref()
                    .map(|a| format!(" (attribution: {})", a))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Returns `Some(source_id)` when `tool_name` looks like an external
/// source tool emitted by [`ExternalSource::tool_name`].
pub fn parse_external_source_tool_name(tool_name: &str) -> Option<&str> {
    tool_name.strip_prefix("source_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::external_source::{
        ExternalSourceAdapter, ExternalSourceDomain, ExternalSourceHttpRequest, ExternalSourceParam,
    };
    use std::collections::BTreeMap;

    fn dummy_source() -> ExternalSource {
        let mut query = BTreeMap::new();
        query.insert("q".to_string(), "{q}".to_string());
        query.insert("limit".to_string(), "{limit}".to_string());
        ExternalSource {
            id: "dummy".to_string(),
            display_name: "Dummy".to_string(),
            description: "Test source".to_string(),
            domain: ExternalSourceDomain::WebSearch,
            adapter: ExternalSourceAdapter::HttpJson,
            review_status: ExternalSourceReviewStatus::AllowedWithConditions,
            review_date: "2026-05-17".to_string(),
            adapter_license: "native".to_string(),
            terms_url: "https://example.com/terms".to_string(),
            review_notes: "ok".to_string(),
            attribution: None,
            credential_policy: ExternalSourceCredentialPolicy::Optional,
            credential_help: None,
            http: ExternalSourceHttpRequest {
                method: "GET".to_string(),
                url: "https://api.example.com/{path}".to_string(),
                query,
                headers: BTreeMap::new(),
                credential_header: Some("X-Token".to_string()),
                credential_prefix: Some("Bearer ".to_string()),
                body_params: Vec::new(),
            },
            params: vec![
                ExternalSourceParam {
                    name: "path".to_string(),
                    description: "path".to_string(),
                    schema: None,
                    required: true,
                    default: None,
                },
                ExternalSourceParam {
                    name: "q".to_string(),
                    description: "query".to_string(),
                    schema: None,
                    required: true,
                    default: None,
                },
                ExternalSourceParam {
                    name: "limit".to_string(),
                    description: "limit".to_string(),
                    schema: None,
                    required: false,
                    default: None,
                },
            ],
            default_pipeline: Vec::new(),
            rate_limit: None,
            mcp_install: None,
        }
    }

    #[test]
    fn substitute_tokens_replaces_known_args_and_drops_unknown() {
        let mut args = serde_json::Map::new();
        args.insert("name".to_string(), Value::String("rust".to_string()));
        let out = substitute_tokens("hello {name}, missing {other}!", &args).unwrap();
        assert_eq!(out, "hello rust, missing !");
    }

    #[test]
    fn build_http_call_for_save_omits_credential_and_skips_blank_query_values() {
        let source = dummy_source();
        let args = serde_json::json!({ "path": "v1/search", "q": "datrina" });
        let (url, headers, body) = build_http_call_for_save(&source, &args).unwrap();
        assert!(url.starts_with("https://api.example.com/v1/search"));
        assert!(url.contains("q=datrina"));
        // {limit} was not provided → no `&limit=` in the query string.
        assert!(
            !url.contains("limit="),
            "blank query values must be skipped: {}",
            url
        );
        assert!(body.is_none());
        // The credential header must NOT be inlined into the saved request.
        let headers_obj = headers.as_object().expect("headers must be an object");
        assert!(
            !headers_obj.contains_key("X-Token"),
            "saved request must not carry a credential header"
        );
    }

    #[test]
    fn build_http_call_rejects_missing_required_param() {
        let source = dummy_source();
        let args = serde_json::json!({ "path": "v1/search" }); // missing "q"
        let result = build_http_call(&source, &args, None);
        let err = result.expect_err("missing required arg must error");
        assert!(
            err.to_string().contains("'q'"),
            "error should name the missing param, got: {}",
            err
        );
    }

    #[test]
    fn build_http_call_with_credential_injects_header_with_prefix() {
        let source = dummy_source();
        let args = serde_json::json!({ "path": "v1/search", "q": "datrina" });
        let (_, headers, _) = build_http_call(&source, &args, Some("abc123")).unwrap();
        let headers_obj = headers.as_object().unwrap();
        assert_eq!(
            headers_obj.get("X-Token").and_then(Value::as_str),
            Some("Bearer abc123")
        );
    }

    #[test]
    fn parse_external_source_tool_name_recognises_prefix() {
        assert_eq!(
            parse_external_source_tool_name("source_hacker_news_search"),
            Some("hacker_news_search")
        );
        assert_eq!(parse_external_source_tool_name("http_request"), None);
        assert_eq!(parse_external_source_tool_name("source_"), Some(""));
    }
}

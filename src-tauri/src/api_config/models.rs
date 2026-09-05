//! Model discovery: `GET {base}/models` against a provider endpoint, with
//! the key resolved one-time input → credential store → environment
//! variable, and tolerant error/body handling for the wild west of
//! OpenAI-compatible servers.

use serde::Serialize;
use tauri::State;

use crate::credentials::{CredentialStore, Creds};

/* ---------------------------- model discovery ---------------------------- */

/// One entry of a provider's `/models` listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Build the model-list URL from a user-entered base.
///
/// The rules are the ones Cherry Studio converged on after discovering that
/// every gateway spells its base differently: trim trailing slashes, add a
/// scheme when missing, and append only `models` when the last path segment
/// is already a version (`/v1`, `/paas/v1`, `/compatible-mode/v1`) — else a
/// blind `/v1/models` yields `/v1/v1/models` on half the internet.
pub(crate) fn models_url(base: &str) -> String {
    let mut s = base.trim().trim_end_matches('/').to_string();
    if s.is_empty() {
        return String::new();
    }
    if !s.contains("://") {
        s = format!("https://{s}");
    }
    let last = s.rsplit('/').next().unwrap_or("");
    let versionish = last.len() >= 2
        && last.as_bytes()[0] == b'v'
        && last[1..].bytes().all(|b| b.is_ascii_digit());
    if versionish {
        format!("{s}/models")
    } else {
        format!("{s}/v1/models")
    }
}

/// Parse the listing defensively: only `id` is required; third-party servers
/// ship `data[]` rows with every other field optional or absent. Anthropic's
/// `display_name` is folded into `name`.
pub(crate) fn parse_models_body(body: &serde_json::Value) -> Vec<RemoteModel> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str())?.to_string();
                    if id.trim().is_empty() {
                        return None;
                    }
                    // display_name first: on Anthropic-style rows `name` is
                    // the model family, `display_name` is the human label.
                    let name = m
                        .get("display_name")
                        .or_else(|| m.get("name"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    Some(RemoteModel { id, name })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pull the human-readable cause out of an error body (nested
/// `error.message` → `error` string → top-level `message`), then fall back
/// to the bare status code.
pub(crate) fn api_error_message(status: u16, body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        let nested = v.get("error").and_then(|e| match e {
            // `error.message` (OpenAI shape) or a bare `error` string
            // (a shape seen from aggregators).
            serde_json::Value::Object(_) => e.get("message").and_then(|m| m.as_str()),
            serde_json::Value::String(s) => Some(s.as_str()),
            _ => None,
        });
        let message = nested.or_else(|| v.get("message").and_then(|m| m.as_str()));
        if let Some(m) = message {
            return format!("端点返回错误 ({status}): {m}");
        }
    }
    format!("端点返回 HTTP {status}")
}

/// `ENV_MISSING:` prefixes the env-var-not-set failure so the frontend can
/// distinguish "ask for a temporary key" from real endpoint errors without
/// matching prose.
pub(crate) const ENV_MISSING: &str = "ENV_MISSING:";

/// `GET {base}/models` with the provider's auth headers.
///
/// The key is resolved in strict priority order: explicit one-time `api_key`
/// (used and discarded — this command never writes it anywhere), then the
/// environment variable the library entry references. PHL as a *desktop*
/// process rarely sees a variable the user only exported in their terminal
/// session, which is why the temp-key escape hatch exists at all.
#[tauri::command]
pub async fn fetch_provider_models(
    creds: State<'_, Creds>,
    base_url: String,
    api: Option<String>,
    api_key_env: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<Vec<RemoteModel>, String> {
    fetch_models_inner(&creds, base_url, api, api_key_env, api_key, provider_id).await
}

pub(crate) async fn fetch_models_inner(
    creds: &Creds,
    base_url: String,
    api: Option<String>,
    api_key_env: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<Vec<RemoteModel>, String> {
    let url = models_url(&base_url);
    if url.is_empty() {
        return Err("未填写 Base URL，无法获取模型列表".into());
    }
    let env_name = api_key_env.trim();
    // An empty variable name is a UI bug, not a missing key: `std::env::var("")`
    // fails everywhere, and the resulting "环境变量  未设置" reads like a
    // formatting accident. Fail with a clear message instead.
    if env_name.is_empty() {
        return Err("未填写密钥环境变量名，且未提供临时密钥".into());
    }
    let key = api_key
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| {
            provider_id
                .as_deref()
                .and_then(|id| creds.get(id).ok().flatten())
                .filter(|v| !v.trim().is_empty())
        })
        .or_else(|| {
            std::env::var(env_name)
                .ok()
                .filter(|v| !v.trim().is_empty())
        })
        .ok_or_else(|| {
            format!("{ENV_MISSING}环境变量 {env_name} 未设置，可临时输入一次密钥（不会被保存）")
        })?;

    let anthropic = api.as_deref() == Some("anthropic");
    let mut request = crate::versions::http_client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(10));
    request = if anthropic {
        request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
    } else {
        request.bearer_auth(key)
    };

    let response = request
        .send()
        .await
        .map_err(|e| format!("无法连接端点: {e}"))?;
    let status = response.status();
    let body: serde_json::Value = if status.is_success() {
        response
            .json()
            .await
            .map_err(|e| format!("模型列表解析失败: {e}"))?
    } else {
        // Read the (usually JSON) error body *before* synthesizing the
        // message — the provider's own text is the only useful half.
        let text = response.text().await.unwrap_or_default();
        return Err(api_error_message(status.as_u16(), &text));
    };
    Ok(parse_models_body(&body))
}

//! The global provider library (`<root>/config/api.json`): load and save.
//!
//! Secrets never persist here: keys typed in the UI move into the OS
//! credential store on save, and a legacy file that still carries plaintext
//! keys is migrated on load and rewritten without them.

use tauri::State;

use super::{api_config_path, valid_env_name, valid_provider_name, ApiConfig};
use crate::credentials::{CredentialStore, Creds};
use crate::paths::PhlState;

#[tauri::command]
pub async fn load_api_config(
    phl: State<'_, PhlState>,
    creds: State<'_, Creds>,
) -> Result<Option<ApiConfig>, String> {
    let path = api_config_path(&phl.root());
    let raw = match tokio::fs::read_to_string(&path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut config: ApiConfig =
        serde_json::from_str(&raw).map_err(|e| format!("API 配置库解析失败: {e}"))?;
    if config.version != 1 {
        return Err(format!("不支持的 API 配置版本: {}", config.version));
    }
    // Migrate plaintext keys (the pre-credential-store format) into the OS
    // store and rewrite the file without them. Failure leaves the old file
    // untouched — an un-migrated plaintext key beats a broken load.
    match migrate_stored_keys(&mut config, &creds) {
        Ok(n) if n > 0 => {
            let body = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
            let tmp = path.with_extension("json.tmp");
            tokio::fs::write(&tmp, body)
                .await
                .map_err(|e| e.to_string())?;
            tokio::fs::rename(&tmp, &path)
                .await
                .map_err(|e| e.to_string())?;
            eprintln!("[phl] 已将 {n} 个密钥迁移到系统凭据管理器，api.json 已去除明文");
        }
        Ok(_) => {}
        Err(e) => eprintln!("[phl] 凭据迁移失败，api.json 中的明文密钥暂保留: {e}"),
    }
    Ok(Some(config))
}

/// Moves every provider's locally stored key into the credential store and
/// clears the plaintext field. Idempotent: an already-migrated config has no
/// keys to move. A store failure aborts before anything is cleared.
fn migrate_stored_keys(config: &mut ApiConfig, creds: &Creds) -> Result<usize, String> {
    let mut migrated = 0;
    for p in &mut config.providers {
        let key = p
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(str::to_string);
        if let Some(key) = key {
            creds.set(&p.id, &key)?;
            p.api_key = None;
            migrated += 1;
        }
    }
    Ok(migrated)
}

#[tauri::command]
pub async fn save_api_config(
    phl: State<'_, PhlState>,
    creds: State<'_, Creds>,
    mut config: ApiConfig,
) -> Result<ApiConfig, String> {
    if config.version != 1 {
        return Err(format!("不支持的 API 配置版本: {}", config.version));
    }
    for p in &config.providers {
        if !valid_provider_name(&p.name) {
            return Err(format!("非法的供应商标识名: {}", p.name));
        }
        if !valid_env_name(&p.api_key_env) {
            return Err(format!(
                "环境变量名不合法（字母/下划线开头，仅含字母数字下划线）: {}",
                p.api_key_env
            ));
        }
    }
    // Keys typed in the UI go to the credential store, never to disk. A
    // store failure must not fall back to writing plaintext: the save fails
    // and the old file stands.
    let migrated = migrate_stored_keys(&mut config, &creds)
        .map_err(|e| format!("无法将密钥写入系统凭据管理器，未保存: {e}"))?;
    // Providers removed from the library take their credentials with them.
    let path = api_config_path(&phl.root());
    if let Ok(old_raw) = tokio::fs::read_to_string(&path).await {
        if let Ok(old) = serde_json::from_str::<ApiConfig>(&old_raw) {
            for op in &old.providers {
                if !config.providers.iter().any(|np| np.id == op.id) {
                    if let Err(e) = creds.delete(&op.id) {
                        eprintln!("[phl] 清理已删除供应商的凭据失败: {e}");
                    }
                }
            }
        }
    }
    config.updated_at = crate::versions::now_iso();
    let _ = migrated;
    let body = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    let path = api_config_path(&phl.root());
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    // Same tmp+rename discipline as `write_manifest`: a crash mid-write must
    // not leave a half file that reads as "no config library".
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| e.to_string())?;
    Ok(config)
}

//! The global provider library (`<root>/config/api.json`): load and save.
//!
//! Secrets never persist here: keys typed in the UI move into the OS
//! credential store on save, and a legacy file that still carries plaintext
//! keys is migrated on load and rewritten without them.
//!
//! A save is a *preparation → commit → cleanup* transaction (O-07): the
//! credential store only gains entries before the file is committed, and the
//! keys of providers removed from the library are deleted only *after* the
//! new file is in place. A failing commit can therefore never strand the old
//! configuration on missing secrets — the worst case is an orphaned store
//! entry for a key the user re-saves, which is invisible and harmless.

use std::path::Path;

use tauri::State;

use super::{api_config_path, valid_env_name, valid_provider_name, ApiConfig};
use crate::credentials::{CredentialStore, Creds};
use crate::paths::PhlState;

#[tauri::command]
pub async fn load_api_config(
    phl: State<'_, PhlState>,
    creds: State<'_, Creds>,
) -> Result<Option<ApiConfig>, String> {
    // One root resolution for the whole load: the path read and the path
    // rewritten after a plaintext migration must be the same file, even if
    // a migration commits the pointer mid-flight.
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
    match migrate_stored_keys(&mut config, &*creds) {
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
fn migrate_stored_keys(
    config: &mut ApiConfig,
    creds: &dyn CredentialStore,
) -> Result<usize, String> {
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
    config: ApiConfig,
) -> Result<ApiConfig, String> {
    let path = api_config_path(&phl.root());
    save_api_config_at(&path, &*creds, config).await
}

/// The command's body, parameterised on the resolved path and a credential
/// store so the commit ordering is testable without the OS manager.
pub(crate) async fn save_api_config_at(
    path: &Path,
    creds: &dyn CredentialStore,
    mut config: ApiConfig,
) -> Result<ApiConfig, String> {
    if config.version != 1 {
        return Err(format!("不支持的 API 配置版本: {}", config.version));
    }
    for p in &config.providers {
        for model in &p.models {
            model.validate_capabilities()?;
        }
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
    // Read the previous file *first*: it names the providers whose credentials
    // may be retired, and retirement only becomes lawful once the commit
    // below has actually replaced the file that referenced them.
    let old: Option<ApiConfig> = tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok());

    // Keys typed in the UI go to the credential store, never to disk. A
    // store failure must not fall back to writing plaintext: the save fails
    // and the old file stands. The step is purely additive — an entry for a
    // provider the old file does not know about is orphaned at worst.
    let _migrated = migrate_stored_keys(&mut config, creds)
        .map_err(|e| format!("无法将密钥写入系统凭据管理器，未保存: {e}"))?;

    config.updated_at = crate::versions::now_iso();
    let body = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    // Same tmp+rename discipline as `write_manifest`: a crash mid-write must
    // not leave a half file that reads as "no config library". A failed
    // commit leaves the OLD file and every credential it references intact.
    let tmp = path.with_extension("json.tmp");
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| format!("API 配置写入失败，旧配置与凭据保持不变: {e}"))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| format!("API 配置写入失败，旧配置与凭据保持不变: {e}"))?;

    // Cleanup phase: providers removed from the library take their
    // credentials with them — but only now that the removal is committed.
    if let Some(old) = old {
        for op in &old.providers {
            if !config.providers.iter().any(|np| np.id == op.id) {
                if let Err(e) = creds.delete(&op.id) {
                    eprintln!("[phl] 清理已删除供应商的凭据失败: {e}");
                }
            }
        }
    }
    Ok(config)
}

/* ------------------------------ tests ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_config::Provider;
    use std::sync::{Arc, Mutex};

    /// Records every store operation so ordering assertions do not need the
    /// real credential manager (and cannot touch it).
    #[derive(Clone, Default)]
    struct FakeStore {
        ops: Arc<Mutex<Vec<String>>>,
        fail_set: Arc<Mutex<Vec<String>>>,
    }

    impl CredentialStore for FakeStore {
        fn get(&self, _id: &str) -> Result<Option<String>, String> {
            Ok(None) // the save path never reads
        }
        fn set(&self, id: &str, _secret: &str) -> Result<(), String> {
            self.ops.lock().unwrap().push(format!("set {id}"));
            if self.fail_set.lock().unwrap().iter().any(|f| f == id) {
                return Err("fake store refusal".into());
            }
            Ok(())
        }
        fn delete(&self, id: &str) -> Result<(), String> {
            self.ops.lock().unwrap().push(format!("delete {id}"));
            Ok(())
        }
    }

    fn provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: id.to_string(),
            kind: "custom".into(),
            notes: None,
            api: None,
            base_url: None,
            api_key_env: format!("{id}_KEY"),
            api_key: None,
            models: vec![],
            enabled: true,
        }
    }

    fn config_with(ids: &[&str]) -> ApiConfig {
        ApiConfig {
            version: 1,
            updated_at: String::new(),
            default_provider_id: None,
            default_model: None,
            providers: ids.iter().map(|i| provider(i)).collect(),
        }
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-apilib-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("api.json")
    }

    #[tokio::test]
    async fn a_failed_commit_never_deletes_referenced_credentials() {
        // `gone` is in the old file but not the new one. Its key must survive
        // if the commit fails — deleting it before the file is replaced is
        // exactly the O-07 bug.
        let path = temp_path("commit-fail");
        std::fs::write(
            &path,
            serde_json::to_string(&config_with(&["kept", "gone"])).unwrap(),
        )
        .unwrap();
        // Force rename to fail: a directory occupies the target's name space
        // in a way rename cannot overwrite.
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();

        let store = FakeStore::default();
        let err = save_api_config_at(&path, &store, config_with(&["kept"]))
            .await
            .expect_err("commit must fail");
        assert!(err.contains("写入失败"), "{err}");
        let ops = store.ops.lock().unwrap();
        assert!(
            !ops.contains(&"delete gone".to_string()),
            "cleanup must not run when the commit failed: {ops:?}"
        );
        drop(ops);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn a_committed_removal_retires_credentials_after_the_rename() {
        let path = temp_path("commit-ok");
        std::fs::write(
            &path,
            serde_json::to_string(&config_with(&["kept", "gone"])).unwrap(),
        )
        .unwrap();
        let store = FakeStore::default();
        save_api_config_at(&path, &store, config_with(&["kept"]))
            .await
            .unwrap();
        let ops = store.ops.lock().unwrap();
        assert_eq!(ops.as_slice(), ["delete gone"]);
        let stored: ApiConfig =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(stored.providers.len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[tokio::test]
    async fn store_failure_aborts_before_any_file_change() {
        let path = temp_path("store-fail");
        let old_body = serde_json::to_string(&config_with(&["kept"])).unwrap();
        std::fs::write(&path, &old_body).unwrap();
        let store = FakeStore::default();
        store.fail_set.lock().unwrap().push("added".into());

        // A brand-new provider carries a typed key; the store refuses it.
        let mut next = config_with(&["kept", "added"]);
        next.providers[1].api_key = Some("sk-secret".into());
        let err = save_api_config_at(&path, &store, next).await.unwrap_err();
        assert!(err.contains("凭据管理器"), "{err}");
        // The library file still says what it said: no partial save.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old_body);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}

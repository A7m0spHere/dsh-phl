//! Environment verification (T-101): a structured, read-only health report
//! for one instance. Verify never modifies the environment — every repair
//! decision is left to the (later) repair flow and the user.
//!
//! The result doubles as the backend half of the health-state model (T-103):
//! `overall` is the Environment Health (healthy/degraded/broken), kept
//! strictly separate from the Process State the launch module owns.

use std::net::TcpListener;
use std::path::Path;

use serde::Serialize;
use tauri::State;

use crate::paths::PhlState;

/// One check in the report. `severity` is what the problem *would mean*
/// (info/warning/error), `status` is what this run found (pass/warn/fail).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyCheck {
    pub id: String,
    pub category: String,
    pub severity: String,
    pub status: String,
    pub message: String,
    pub repairable: bool,
    pub repair_action: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    /// healthy | degraded | broken — the Environment Health.
    pub overall: String,
    pub checks: Vec<VerifyCheck>,
}

impl VerifyCheck {
    fn pass(id: &str, category: &str, message: String) -> Self {
        Self {
            id: id.into(),
            category: category.into(),
            severity: "info".into(),
            status: "pass".into(),
            message,
            repairable: false,
            repair_action: None,
        }
    }

    fn warn(
        id: &str,
        category: &str,
        message: String,
        repairable: bool,
        action: Option<&str>,
    ) -> Self {
        Self {
            id: id.into(),
            category: category.into(),
            severity: "warning".into(),
            status: "warn".into(),
            message,
            repairable,
            repair_action: action.map(Into::into),
        }
    }

    fn fail(
        id: &str,
        category: &str,
        message: String,
        repairable: bool,
        action: Option<&str>,
    ) -> Self {
        Self {
            id: id.into(),
            category: category.into(),
            severity: "error".into(),
            status: "fail".into(),
            message,
            repairable,
            repair_action: action.map(Into::into),
        }
    }
}

fn worst(checks: &[VerifyCheck]) -> String {
    if checks.iter().any(|c| c.status == "fail") {
        "broken".into()
    } else if checks.iter().any(|c| c.status == "warn") {
        "degraded".into()
    } else {
        "healthy".into()
    }
}

#[tauri::command]
pub async fn verify_instance(
    phl: State<'_, PhlState>,
    instance_id: String,
) -> Result<VerifyResult, String> {
    verify_instance_inner(&phl.root(), &instance_id).await
}

pub(crate) async fn verify_instance_inner(root: &Path, id: &str) -> Result<VerifyResult, String> {
    let mut checks: Vec<VerifyCheck> = Vec::new();
    let dir = crate::instances::instance_dir(root, id)?;
    let manifest = match crate::instances::load_manifest(&dir, id).await {
        Ok(m) => m,
        Err(e) => {
            // Without a readable manifest nothing else can be judged.
            checks.push(VerifyCheck::fail("manifest", "manifest", e, false, None));
            return Ok(VerifyResult {
                overall: worst(&checks),
                checks,
            });
        }
    };
    checks.push(VerifyCheck::pass(
        "manifest",
        "manifest",
        format!("清单可读（schema {}）", manifest.schema_version),
    ));

    checks.extend(check_dsh(root, &manifest).await);
    checks.extend(check_runtime(root, &manifest).await);
    checks.extend(check_config(&dir).await);
    checks.extend(check_api(root, &manifest).await);
    checks.extend(check_launch(&dir, &manifest).await);
    checks.push(check_writable(&dir).await);
    Ok(VerifyResult {
        overall: worst(&checks),
        checks,
    })
}

async fn check_dsh(root: &Path, manifest: &crate::instances::InstanceManifest) -> Vec<VerifyCheck> {
    let mut out = Vec::new();
    let bare = manifest.version_id.trim_start_matches("dsh-");
    match crate::versions::sanitize_version(bare) {
        Err(e) => {
            out.push(VerifyCheck::fail("dsh-version", "dsh", e, false, None));
            return out;
        }
        Ok(name) => {
            let dir = root.join("versions").join(&name);
            if crate::versions::read_marker(&dir).await.is_none() {
                out.push(VerifyCheck::fail(
                    "dsh-version",
                    "dsh",
                    format!("实例指向的 DSH 版本 {name} 未安装"),
                    true,
                    Some("install-version"),
                ));
                // Nothing further to inspect without the tree.
                return out;
            }
            out.push(VerifyCheck::pass(
                "dsh-version",
                "dsh",
                format!("DSH {name} 已安装"),
            ));
            // Tree integrity: same expectations the transactional installer's
            // health gate enforces, so a degraded tree cannot hide here.
            let entry_ok = dir.join("package.json").exists();
            let bin_ok = dir.join("lib").join("bin.js").exists();
            let deps_ok =
                !crate::versions::package_requires_deps(&dir) || dir.join("node_modules").exists();
            if entry_ok && bin_ok && deps_ok {
                out.push(VerifyCheck::pass(
                    "dsh-tree",
                    "dsh",
                    "package.json、lib/bin.js 与 node_modules 完整".into(),
                ));
            } else {
                let missing = [
                    (!entry_ok).then_some("package.json"),
                    (!bin_ok).then_some("lib/bin.js"),
                    (!deps_ok).then_some("node_modules"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("、");
                out.push(VerifyCheck::fail(
                    "dsh-tree",
                    "dsh",
                    format!("版本目录不完整，缺少 {missing}"),
                    true,
                    Some("reinstall-version"),
                ));
            }
            // Dependency-pruning status, exactly as the installer recorded it.
            let raw = std::fs::read_to_string(dir.join("phl-deps.json")).unwrap_or_default();
            #[derive(serde::Deserialize, Default)]
            #[serde(rename_all = "camelCase")]
            struct DepsMarker {
                #[serde(default)]
                install_health: String,
                #[serde(default)]
                skipped: Vec<String>,
            }
            let deps: DepsMarker = serde_json::from_str(&raw).unwrap_or_default();
            if deps.install_health == "degraded" {
                out.push(VerifyCheck::warn(
                    "dsh-deps",
                    "dsh",
                    format!("安装时跳过依赖：{}", deps.skipped.join("、")),
                    false,
                    None,
                ));
            }
        }
    }
    out
}

async fn check_runtime(
    root: &Path,
    manifest: &crate::instances::InstanceManifest,
) -> Vec<VerifyCheck> {
    let mut out = Vec::new();
    if manifest.runtime_id == "node-system" {
        out.push(VerifyCheck::pass(
            "runtime",
            "runtime",
            "使用系统 Node（由系统管理）".into(),
        ));
        return out;
    }
    // The runtime id doubles as the directory name under <root>/runtimes.
    let dir = root.join("runtimes").join(&manifest.runtime_id);
    let Some(recorded) = crate::runtimes::runtime_version(&dir) else {
        out.push(VerifyCheck::fail(
            "runtime",
            "runtime",
            format!("实例指向的 Runtime {} 未安装", manifest.runtime_id),
            true,
            Some("install-runtime"),
        ));
        return out;
    };
    out.push(VerifyCheck::pass(
        "runtime",
        "runtime",
        format!("Runtime {} 已安装（v{recorded}）", manifest.runtime_id),
    ));

    // The binary must exist *and* run, and report exactly the recorded
    // version — the same gate the runtime installer enforces before a swap.
    let bin = if cfg!(windows) { dir } else { dir.join("bin") };
    let node = bin.join(crate::launch::node_binary());
    if !node.exists() {
        out.push(VerifyCheck::fail(
            "runtime-binary",
            "runtime",
            "node 可执行文件缺失".to_string(),
            true,
            Some("reinstall-runtime"),
        ));
        return out;
    }
    let probe = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(&node);
        command.arg("--version");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(crate::launch::CREATE_NO_WINDOW);
        }
        command
            .output()
            .map_err(|e| format!("无法运行 node --version: {e}"))
    })
    .await
    .map_err(|e| format!("校验线程异常退出: {e}"));
    match probe {
        Ok(Ok(output)) if output.status.success() => {
            let reported = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if reported == format!("v{recorded}") {
                out.push(VerifyCheck::pass(
                    "runtime-binary",
                    "runtime",
                    format!("node --version 返回 v{recorded}，与标记一致"),
                ));
            } else {
                out.push(VerifyCheck::warn(
                    "runtime-binary",
                    "runtime",
                    format!("node --version 返回 {reported}，与标记 v{recorded} 不一致"),
                    true,
                    Some("reinstall-runtime"),
                ));
            }
        }
        Ok(Ok(output)) => out.push(VerifyCheck::fail(
            "runtime-binary",
            "runtime",
            format!(
                "node --version 退出码 {}",
                output.status.code().unwrap_or(-1)
            ),
            true,
            Some("reinstall-runtime"),
        )),
        Ok(Err(e)) | Err(e) => out.push(VerifyCheck::fail(
            "runtime-binary",
            "runtime",
            e,
            false,
            None,
        )),
    }
    out
}

async fn check_config(dir: &Path) -> Vec<VerifyCheck> {
    let mut out = Vec::new();
    let dsh_home = dir.join("dsh-home");
    let settings = dsh_home.join("settings.yaml");
    if settings.exists() {
        match tokio::fs::read_to_string(&settings).await {
            Ok(raw) => match serde_yaml::from_str::<serde_yaml::Value>(&raw) {
                Ok(_) => out.push(VerifyCheck::pass(
                    "config-settings",
                    "config",
                    "settings.yaml 可解析".into(),
                )),
                Err(e) => out.push(VerifyCheck::fail(
                    "config-settings",
                    "config",
                    format!("settings.yaml 解析失败: {e}"),
                    false,
                    None,
                )),
            },
            Err(e) => out.push(VerifyCheck::fail(
                "config-settings",
                "config",
                format!("settings.yaml 读取失败: {e}"),
                false,
                None,
            )),
        }
    } else {
        // DSH writes it on first launch; absence is normal, not damage.
        out.push(VerifyCheck::pass(
            "config-settings",
            "config",
            "settings.yaml 尚未生成（首次启动时由 DSH 创建）".into(),
        ));
    }
    out
}

async fn check_api(root: &Path, manifest: &crate::instances::InstanceManifest) -> Vec<VerifyCheck> {
    let Some(binding) = &manifest.api else {
        return vec![VerifyCheck::pass(
            "api-binding",
            "api",
            "实例未由 PHL 托管 API 配置".into(),
        )];
    };
    if binding.inheritance == "none" {
        return vec![VerifyCheck::pass(
            "api-binding",
            "api",
            "实例完全自管 API 配置".into(),
        )];
    }
    if binding.provider_ids.is_empty() {
        return vec![VerifyCheck::pass(
            "api-binding",
            "api",
            "继承全局供应商库".into(),
        )];
    }
    // Only provider *ids* are checked — neither file ever holds secret
    // material (the library stores env-var names, not keys).
    let raw = tokio::fs::read_to_string(root.join("config").join("api.json"))
        .await
        .unwrap_or_default();
    #[derive(serde::Deserialize)]
    struct ProviderId {
        #[serde(default, rename = "id")]
        id: String,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Library {
        #[serde(default)]
        providers: Vec<ProviderId>,
    }
    let known: Vec<String> = serde_json::from_str::<Library>(&raw)
        .map(|lib| lib.providers.into_iter().map(|p| p.id).collect())
        .unwrap_or_default();
    let missing: Vec<String> = binding
        .provider_ids
        .iter()
        .filter(|p| !known.contains(p))
        .cloned()
        .collect();
    if missing.is_empty() {
        vec![VerifyCheck::pass(
            "api-binding",
            "api",
            format!("引用的 {} 个供应商都在全局库中", binding.provider_ids.len()),
        )]
    } else {
        vec![VerifyCheck::warn(
            "api-binding",
            "api",
            format!("供应商已不在全局库中：{}", missing.join("、")),
            false,
            None,
        )]
    }
}

async fn check_launch(
    dir: &Path,
    manifest: &crate::instances::InstanceManifest,
) -> Vec<VerifyCheck> {
    let mut out = Vec::new();
    if manifest.port == 0 || manifest.port > 65535 {
        out.push(VerifyCheck::fail(
            "launch-port",
            "launch",
            format!("端口 {} 非法", manifest.port),
            false,
            None,
        ));
    } else {
        let port = manifest.port;
        let free = tokio::task::spawn_blocking(move || {
            TcpListener::bind(("127.0.0.1", port as u16)).is_ok()
        })
        .await
        .unwrap_or(true);
        if free {
            out.push(VerifyCheck::pass(
                "launch-port",
                "launch",
                format!("端口 {} 可用", manifest.port),
            ));
        } else {
            out.push(VerifyCheck::warn(
                "launch-port",
                "launch",
                format!("端口 {} 当前被占用（实例运行时属正常）", manifest.port),
                false,
                None,
            ));
        }
    }
    if dir.join("workspace").exists() {
        out.push(VerifyCheck::pass(
            "launch-workspace",
            "launch",
            "workspace 目录存在".into(),
        ));
    } else {
        out.push(VerifyCheck::warn(
            "launch-workspace",
            "launch",
            "workspace 目录缺失".to_string(),
            true,
            Some("recreate-workspace"),
        ));
    }
    out
}

async fn check_writable(dir: &Path) -> VerifyCheck {
    let probe = dir.join(".phl-verify-probe");
    match tokio::fs::write(&probe, b"ok").await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(&probe).await;
            VerifyCheck::pass("files-writable", "config", "实例目录可写".into())
        }
        Err(e) => VerifyCheck::fail(
            "files-writable",
            "config",
            format!("实例目录不可写: {e}"),
            false,
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::{create_instance_inner, InstanceManifest};
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-verify-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn manifest(id: &str, name: &str) -> InstanceManifest {
        InstanceManifest {
            schema_version: 0,
            id: id.into(),
            name: name.into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-22".into(),
            port: 3080,
            auto_port: true,
            profile: "web".into(),
            created_at: crate::versions::now_iso(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: HashMap::new(),
            args: Vec::new(),
            api: None,
        }
    }

    fn check<'a>(result: &'a VerifyResult, id: &str) -> &'a VerifyCheck {
        result.checks.iter().find(|c| c.id == id).unwrap()
    }

    #[tokio::test]
    async fn a_fresh_instance_verifies_healthy() {
        let root = temp_root("fresh");
        let id = "fresh-0001";
        let mut m = manifest(id, "Fresh");
        // The system-node branch skips the runtime-tree probe, keeping this
        // test about the DSH half.
        m.runtime_id = "node-system".into();
        create_instance_inner(&root, m).await.unwrap();

        // A freshly created instance already points at an installed version —
        // the wizard requires one — so simulate that completed install.
        let version = root.join("versions").join("0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::write(version.join("package.json"), r#"{"name":"dsh"}"#).unwrap();
        std::fs::write(version.join("lib").join("bin.js"), "// bin").unwrap();
        std::fs::write(
            version.join("phl-install.json"),
            serde_json::json!({"installedAt": crate::versions::now_iso(), "version": "0.1.0"})
                .to_string(),
        )
        .unwrap();

        let result = verify_instance_inner(&root, id).await.unwrap();
        assert_eq!(result.overall, "healthy", "{:?}", result.checks);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_missing_dsh_version_is_broken_and_marked_repairable() {
        let root = temp_root("missing-dsh");
        let id = "missing-01";
        let mut m = manifest(id, "Missing");
        m.port = 34473;
        create_instance_inner(&root, m).await.unwrap();

        let result = verify_instance_inner(&root, id).await.unwrap();
        assert_eq!(result.overall, "broken");
        let dsh = check(&result, "dsh-version");
        assert_eq!(dsh.status, "fail");
        assert!(dsh.repairable);
        assert_eq!(dsh.repair_action.as_deref(), Some("install-version"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_installed_version_with_a_complete_tree_passes_and_a_degraded_marker_warns() {
        let root = temp_root("dsh-tree");
        let id = "dsh-tree-01";
        let mut m = manifest(id, "Tree");
        m.port = 34471; // 独立端口：并行测试同时探测 3080 会互相报占用
        m.runtime_id = "node-system".into();
        create_instance_inner(&root, m).await.unwrap();

        // Simulate a completed DSH install with its own node_modules.
        let version = root.join("versions").join("0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::create_dir_all(version.join("node_modules")).unwrap();
        std::fs::write(
            version.join("package.json"),
            r#"{"name":"dsh","dependencies":{"commander":"^15.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(version.join("lib").join("bin.js"), "// bin").unwrap();
        std::fs::write(
            version.join("phl-install.json"),
            serde_json::json!({"installedAt": crate::versions::now_iso(), "version": "0.1.0"})
                .to_string(),
        )
        .unwrap();

        let result = verify_instance_inner(&root, id).await.unwrap();
        assert_eq!(
            check(&result, "dsh-version").status,
            "pass",
            "{:?}",
            result.checks
        );
        assert_eq!(
            check(&result, "dsh-tree").status,
            "pass",
            "{:?}",
            result.checks
        );
        assert_eq!(result.overall, "healthy", "{:?}", result.checks);

        // The installer's degraded record surfaces as a warning.
        std::fs::write(
            version.join("phl-deps.json"),
            serde_json::json!({"installHealth": "degraded", "skipped": ["@deepseek-ai/dsh-experimental-x"]}).to_string(),
        )
        .unwrap();
        let result = verify_instance_inner(&root, id).await.unwrap();
        assert_eq!(check(&result, "dsh-deps").status, "warn");
        assert!(check(&result, "dsh-deps")
            .message
            .contains("dsh-experimental-x"));
        assert_eq!(result.overall, "degraded");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_broken_settings_file_makes_the_instance_broken() {
        let root = temp_root("settings");
        let id = "settings-1";
        let mut m = manifest(id, "Cfg");
        m.port = 34472;
        m.runtime_id = "node-system".into();
        create_instance_inner(&root, m).await.unwrap();
        // An installed version keeps the report focused on the config check.
        let version = root.join("versions").join("0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::write(version.join("package.json"), r#"{"name":"dsh"}"#).unwrap();
        std::fs::write(version.join("lib").join("bin.js"), "// bin").unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();

        let dir = root.join("instances").join(id);
        std::fs::write(dir.join("dsh-home").join("settings.yaml"), "{ not yaml: [").unwrap();
        let result = verify_instance_inner(&root, id).await.unwrap();
        assert_eq!(check(&result, "config-settings").status, "fail");
        assert_eq!(result.overall, "broken");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_missing_runtime_is_broken_and_marked_repairable() {
        let root = temp_root("missing-runtime");
        let id = "no-runtime";
        let mut m = manifest(id, "NoRuntime");
        m.port = 34474;
        // runtime_id stays the helper's "node-22": no tree exists for it.
        create_instance_inner(&root, m).await.unwrap();
        // Install a DSH version so only the runtime check can fail.
        let version = root.join("versions").join("0.1.0");
        std::fs::create_dir_all(version.join("lib")).unwrap();
        std::fs::write(version.join("package.json"), r#"{"name":"dsh"}"#).unwrap();
        std::fs::write(version.join("lib").join("bin.js"), "// bin").unwrap();
        std::fs::write(version.join("phl-install.json"), "{}").unwrap();

        let result = verify_instance_inner(&root, id).await.unwrap();
        let runtime = check(&result, "runtime");
        assert_eq!(runtime.status, "fail");
        assert!(runtime.repairable);
        assert_eq!(runtime.repair_action.as_deref(), Some("install-runtime"));
        assert_eq!(result.overall, "broken");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_unreadable_manifest_reports_broken_with_a_single_check() {
        let root = temp_root("no-manifest");
        let id = "ghost-0001";
        // No create: the instance directory simply does not exist.
        let result = verify_instance_inner(&root, id).await.unwrap();
        assert_eq!(result.overall, "broken");
        assert_eq!(result.checks.len(), 1);
        assert_eq!(result.checks[0].id, "manifest");

        let _ = std::fs::remove_dir_all(&root);
    }
}

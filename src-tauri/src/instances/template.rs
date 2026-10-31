//! Reusable instance-environment templates (Issue #13).
//!
//! A template is a REPRODUCTION LIST distilled from one managed instance:
//! the target DSH version, runtime binding, allowed non-sensitive settings,
//! and the PHL-managed plugin list with exact versions, enable states and —
//! recorded at export time — each package's own `engines.dsh` claim, so the
//! create-time preview can apply issue #10's rule OFFLINE. It is not a
//! backup: no plugin files, no sessions/attachments/workspace, and no
//! credential values ever enter the file (issue #11's wording rules apply:
//! UI copy says 实例模板, never 备份).
//!
//! Reproduction runs through EXISTING machinery only: instance creation is
//! the regular `create_instance`, plugin installation reuses the regular
//! install transaction per plugin (issue #13: 不再造一套安装系统), and the
//! credential/machine-local env filtering is the SAME `partition_env` the
//! Bundle exporter uses — never a second filter implementation.

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::env_policy::partition_env;

/// Bumped only for a breaking file-format change; readers refuse newer
/// formats explicitly (确定行为, issue #13 acceptance).
pub(crate) const TEMPLATE_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TemplateEnvEntry {
    pub key: String,
    /// `Some` = a non-sensitive value that reproduces verbatim; `None` = a
    /// credential-carrier or machine-local name recorded WITHOUT its value.
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePluginEntry {
    pub plugin_id: String,
    pub registry_id: String,
    pub version: String,
    pub enabled: bool,
    /// The install source recorded in the PHL marker; `None` = the marker
    /// predates source recording, and the plugin is honestly reported as
    /// not automatically reproducible.
    pub source: Option<crate::plugins::PluginSourceWire>,
    /// `recorded` | `missing` — decided at export, kept verbatim by preview.
    pub source_status: String,
    /// The package's own `engines.dsh` claim read from the SOURCE instance's
    /// package.json at export time; drives the offline preview verdict.
    pub engine_range: Option<String>,
}

/// The template file (issue #13). Deserialize-compatible with what the
/// exporter writes; the importer refuses newer format versions explicitly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceTemplateFile {
    pub format_version: u32,
    pub exported_at: String,
    pub name: String,
    pub description: String,
    pub kind: String,
    /// Bare DSH version ("0.1.5-rc.3"); empty = 未绑定 (preview blocks it).
    pub dsh_version: String,
    pub runtime_id: String,
    pub args: Vec<String>,
    pub env: Vec<TemplateEnvEntry>,
    pub plugins: Vec<TemplatePluginEntry>,
    /// Plugins seen on the source instance but NOT carried (bundles, local
    /// links) — each with its reason, so nothing disappears silently.
    pub excluded_plugins: Vec<String>,
}

/// Read-only create-time preview (issue #13: 预览来源、下载需求、插件兼容性
/// 与缺失依赖).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceTemplatePreview {
    /// Structural problems that make creation refused (缺版本、过新格式等).
    pub blocked: Vec<String>,
    pub dsh_version_installed: bool,
    pub runtime_installed: bool,
    pub plugins: Vec<TemplatePluginPreview>,
    /// Credential re-configuration, machine-local drops, exclusions.
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePluginPreview {
    pub plugin_id: String,
    pub registry_id: String,
    pub version: String,
    pub enabled: bool,
    pub source_status: String,
    /// engines.dsh verdict against the TEMPLATE's DSH version (issue #10's
    /// rule, applied offline from the claim recorded at export).
    pub engine_verdict: String,
    pub engine_reason: String,
}

/* ------------------------------- export ------------------------------- */

fn full_marker_json(pkg_dir: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(pkg_dir.join("phl-plugin.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Distill a managed instance into a template file. Read-only on the source.
/// `dest` (the user's save-dialog choice, unconfined like the Bundle's) makes
/// it also write the file — with the SAME seal-and-reopen verification the
/// Bundle exporter gained in issue #11.
pub(crate) async fn export_template_inner(
    root: &Path,
    instance_id: &str,
    dest: Option<String>,
) -> Result<InstanceTemplateFile, String> {
    let dir = crate::instances::instance_dir(root, instance_id)?;
    let manifest = crate::instances::manifest::load_manifest(&dir, instance_id).await?;
    let profile = crate::instances::profile_dir(root, instance_id).await?;

    // Env: the Bundle exporter's filter, verbatim. Non-sensitive values may
    // travel; credential carriers and machine-local names keep their key
    // with value = None (issue #13's exclusion list, zero second filter).
    let credential_envs: HashSet<String> = crate::api_config::credential_env_names(root).await;
    let partitioned = partition_env(manifest.env.clone(), &credential_envs);
    let mut env: Vec<TemplateEnvEntry> = partitioned
        .env
        .into_iter()
        .map(|(key, value)| TemplateEnvEntry {
            key,
            value: Some(value),
        })
        .collect();
    for key in partitioned
        .credentials
        .iter()
        .chain(partitioned.machine_only.iter())
    {
        env.push(TemplateEnvEntry {
            key: key.clone(),
            value: None,
        });
    }
    env.sort_by(|a, b| a.key.cmp(&b.key));

    // Plugins: only PHL-managed ones are carried. The full marker JSON holds
    // the recorded install source + (via package.json) the engines.dsh claim.
    let scanned = crate::instances::scan_plugins(&profile).await;
    let mut plugins = Vec::new();
    let mut excluded = Vec::new();
    for p in scanned {
        let pkg_dir = profile.join("node_modules").join(&p.registry_id);
        if !p.manageable || p.linked {
            excluded.push(format!(
                "{id}：{reason}（不由 PHL 安装事务管理，不随模板携带）",
                id = p.registry_id,
                reason = if p.linked {
                    "本地链接"
                } else {
                    "DSH bundle"
                }
            ));
            continue;
        }
        let marker = full_marker_json(&pkg_dir);
        let source = marker.as_ref().and_then(|m| m.get("source")).and_then(|v| {
            serde_json::from_value::<crate::plugins::PluginSourceWire>(v.clone()).ok()
        });
        let source_status = if source.is_some() {
            "recorded"
        } else {
            "missing"
        };
        let engine_range = crate::plugins::engines::package_engine_range(&pkg_dir).await;
        plugins.push(TemplatePluginEntry {
            plugin_id: p.plugin_id,
            registry_id: p.registry_id,
            version: p.version,
            enabled: p.enabled,
            source,
            source_status: source_status.into(),
            engine_range,
        });
    }

    let file = InstanceTemplateFile {
        format_version: TEMPLATE_FORMAT_VERSION,
        exported_at: crate::versions::now_iso(),
        name: format!("{} 的环境模板", manifest.name),
        description: format!("提取自受管实例 {instance_id}；不包含会话、附件、工作区与凭据值"),
        kind: manifest.kind.clone(),
        dsh_version: manifest
            .version_id
            .strip_prefix("dsh-")
            .unwrap_or(&manifest.version_id)
            .to_string(),
        runtime_id: manifest.runtime_id.clone(),
        args: manifest.args.clone(),
        env,
        plugins,
        excluded_plugins: excluded,
    };

    if let Some(dest) = dest {
        let body = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
        std::fs::write(&dest, &body).map_err(|e| format!("无法写入模板文件: {e}"))?;
        // Seal and re-open (issue #11's rule, same as the Bundle exporter):
        // never hand the user a file our own reader refuses.
        let written = std::fs::read(&dest).map_err(|e| format!("无法回读模板文件: {e}"))?;
        if serde_json::from_slice::<InstanceTemplateFile>(&written).is_err() {
            let _ = std::fs::remove_file(&dest);
            return Err("回读校验失败：导出的模板无法被 PHL 自身解析，已删除不完整的导出".into());
        }
    }
    Ok(file)
}

/// Command wrapper: same extraction, `dest` from the frontend save dialog.
#[tauri::command]
pub async fn export_instance_template(
    phl: State<'_, crate::paths::PhlState>,
    instance_id: String,
    dest: Option<String>,
) -> Result<InstanceTemplateFile, String> {
    export_template_inner(&phl.root(), &instance_id, dest).await
}

/* ---------------------------- read & preview --------------------------- */

/// Read a template file. Newer format versions are refused explicitly —
/// the behaviour is deterministic, never a best-effort parse (issue #13
/// acceptance: 重复导入/过新格式行为确定).
#[tauri::command]
pub async fn read_instance_template(path: String) -> Result<InstanceTemplateFile, String> {
    read_template_from(Path::new(&path))
}

pub(crate) fn read_template_from(path: &Path) -> Result<InstanceTemplateFile, String> {
    let raw = std::fs::read(path).map_err(|e| format!("无法读取模板文件: {e}"))?;
    let file: InstanceTemplateFile =
        serde_json::from_slice(&raw).map_err(|e| format!("模板文件无法解析: {e}"))?;
    if file.format_version > TEMPLATE_FORMAT_VERSION {
        return Err(format!(
            "不支持的模板格式版本 {}（当前支持 {}）——请先升级 PHL",
            file.format_version, TEMPLATE_FORMAT_VERSION
        ));
    }
    Ok(file)
}

/// The offline create-time preview: what exists, what would be downloaded,
/// what each plugin's own engines.dsh claim says against the TEMPLATE's DSH
/// version, and every exclusion — nothing guessed, nothing hidden.
pub(crate) async fn preview_template_inner(
    root: &Path,
    template: &InstanceTemplateFile,
) -> Result<InstanceTemplatePreview, String> {
    let mut preview = InstanceTemplatePreview {
        blocked: Vec::new(),
        dsh_version_installed: false,
        runtime_installed: false,
        plugins: Vec::new(),
        notes: Vec::new(),
    };

    if template.dsh_version.trim().is_empty() {
        preview.blocked.push(
            "模板未绑定 DSH 版本（导出源未绑定或文件被改动）；请改用其他模板或手动创建".into(),
        );
    } else {
        preview.dsh_version_installed = root
            .join("versions")
            .join(&template.dsh_version)
            .join("phl-install.json")
            .exists();
        if !preview.dsh_version_installed {
            preview.notes.push(format!(
                "DSH {} 未安装，创建前需先在版本页安装（模板创建不会自动联网下载）",
                template.dsh_version
            ));
        }
    }
    preview.runtime_installed = template.runtime_id == "node-system"
        || root
            .join("runtimes")
            .join(&template.runtime_id)
            .join("phl-runtime.json")
            .exists();
    if !preview.runtime_installed {
        preview.notes.push(format!(
            "Runtime {} 未安装，创建前需先在 Runtime 页安装",
            template.runtime_id
        ));
    }

    let dsh = semver::Version::parse(template.dsh_version.trim()).ok();
    for p in &template.plugins {
        let (verdict, reason) =
            crate::plugins::engines::evaluate(p.engine_range.as_deref(), dsh.as_ref());
        preview.plugins.push(TemplatePluginPreview {
            plugin_id: p.plugin_id.clone(),
            registry_id: p.registry_id.clone(),
            version: p.version.clone(),
            enabled: p.enabled,
            source_status: p.source_status.clone(),
            engine_verdict: verdict.as_str().into(),
            engine_reason: reason,
        });
    }
    for p in template
        .plugins
        .iter()
        .filter(|p| p.source_status == "missing")
    {
        preview.notes.push(format!(
            "{}：安装记录缺少来源信息，无法自动重装，请在插件市场手动安装",
            p.registry_id
        ));
    }
    for note in &template.excluded_plugins {
        preview.notes.push(note.clone());
    }
    if template.env.iter().any(|e| e.value.is_none()) {
        preview
            .notes
            .push("模板中的凭据载体与机器本地变量只保留了键名；创建后需重新配置值".into());
    }
    Ok(preview)
}

/// Command wrapper: same preview over an already-read template object.
#[tauri::command]
pub async fn preview_instance_template(
    phl: State<'_, crate::paths::PhlState>,
    template: InstanceTemplateFile,
) -> Result<InstanceTemplatePreview, String> {
    preview_template_inner(&phl.root(), &template).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::manifest::{write_manifest, InstanceManifest, ManagementMode};

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "phl-template-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("instances")).unwrap();
        dir
    }

    fn manifest(id: &str) -> InstanceManifest {
        let mut m: InstanceManifest = serde_json::from_value(serde_json::json!({
            "schemaVersion": 2,
            "id": id,
            "name": "模板源实例",
            "kind": "development",
            "hue": 0,
            "versionId": "dsh-0.1.5",
            "runtimeId": "node-system",
            "port": 0,
            "autoPort": true,
            "profile": "web",
            "createdAt": "now",
            "env": {
                "HTTP_PROXY": "http://proxy:8080",
                "MY_API_KEY": "should-not-travel-value",
                "DSH_HOME": "C:\\elsewhere",
                "NODE_OPTIONS": "--insecure"
            },
            "args": ["--verbose"]
        }))
        .unwrap();
        m.management_mode = ManagementMode::ManagedCopy;
        m
    }

    /// A PHL-managed plugin installed through the REAL commit path, with an
    /// engines.dsh claim in its package.json and a recorded npm source.
    async fn install_fixture(root: &Path, instance_id: &str, registry_id: &str) {
        let profile = crate::instances::profile_dir(root, instance_id)
            .await
            .unwrap();
        let pkg = profile.join("node_modules").join(registry_id);
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            format!(r#"{{"name":"{registry_id}","version":"1.4.0","engines":{{"dsh":"^0.1.0"}}}}"#),
        )
        .unwrap();
        crate::plugins::install::commit_install(
            &pkg,
            &serde_json::json!({
                "installedAt": "now",
                "pluginId": format!("x/{registry_id}"),
                "version": "1.4.0",
                "kind": "npm",
                "registryId": registry_id,
                "trust": "verified",
                "source": {"kind": "npm", "pkg": registry_id},
            }),
            &profile,
            registry_id,
            &crate::resources::Tasks::default()
                .begin(
                    crate::resources::TaskInfo::new(
                        "template-fixture".into(),
                        "plugin-install",
                        "t".into(),
                        &[],
                    ),
                    None,
                )
                .unwrap(),
            None,
            &crate::plugins::deps::DependencyEnv::new(
                std::path::Path::new(""),
                "node-system",
                None,
            ),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn export_filters_env_and_records_plugin_sources() {
        let root = temp_root("export");
        let id = "tpl-src1";
        let dir = crate::instances::instance_dir(&root, id).unwrap();
        std::fs::create_dir_all(dir.join("profiles").join("web")).unwrap();
        write_manifest(&dir, &manifest(id)).await.unwrap();
        install_fixture(&root, id, "dsh-foo").await;

        let file = super::export_template_inner(&root, id, None).await.unwrap();

        // Env: ordinary value travels; credential + machine-local keep only
        // the key. The value string appears NOWHERE in the file.
        let proxy = file.env.iter().find(|e| e.key == "HTTP_PROXY").unwrap();
        assert_eq!(proxy.value.as_deref(), Some("http://proxy:8080"));
        let key_entry = file.env.iter().find(|e| e.key == "MY_API_KEY").unwrap();
        assert_eq!(key_entry.value, None);
        // 机器本地变量同样只留键名（隔离边界与注入向量不随模板走）。
        assert_eq!(
            file.env.iter().find(|e| e.key == "DSH_HOME").unwrap().value,
            None
        );
        assert_eq!(
            file.env
                .iter()
                .find(|e| e.key == "NODE_OPTIONS")
                .unwrap()
                .value,
            None
        );
        let serialized = serde_json::to_string(&file).unwrap();
        assert!(!serialized.contains("should-not-travel-value"));
        assert!(!serialized.contains("C:\\\\elsewhere"));

        // Plugin: version/enabled recorded, source extracted from the
        // marker, engines.dsh claim captured for the offline preview.
        let plugin = &file.plugins[0];
        assert_eq!(plugin.registry_id, "dsh-foo");
        assert_eq!(plugin.version, "1.4.0");
        assert!(plugin.enabled);
        assert_eq!(plugin.source_status, "recorded");
        assert!(plugin.source.is_some());
        assert_eq!(plugin.engine_range.as_deref(), Some("^0.1.0"));

        // args + kind ride along; the file names itself a template.
        assert_eq!(file.args, vec!["--verbose".to_string()]);
        assert_eq!(file.kind, "development");
        assert!(file.name.contains("环境模板"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn template_file_roundtrips_and_refuses_newer_formats() {
        let root = temp_root("round");
        let path = root.join("t.phl-template.json");
        let file = InstanceTemplateFile {
            format_version: TEMPLATE_FORMAT_VERSION,
            exported_at: "now".into(),
            name: "测试模板".into(),
            description: "d".into(),
            kind: "sandbox".into(),
            dsh_version: "0.1.5".into(),
            runtime_id: "node-system".into(),
            args: vec![],
            env: vec![TemplateEnvEntry {
                key: "A".into(),
                value: Some("b".into()),
            }],
            plugins: vec![TemplatePluginEntry {
                plugin_id: "x/p".into(),
                registry_id: "p".into(),
                version: "1.0.0".into(),
                enabled: false,
                source: None,
                source_status: "missing".into(),
                engine_range: None,
            }],
            excluded_plugins: vec!["bundle-thing：DSH bundle".into()],
        };
        std::fs::write(&path, serde_json::to_vec_pretty(&file).unwrap()).unwrap();
        let back = read_template_from(&path).unwrap();
        assert_eq!(back, file);

        let newer = r#"{"formatVersion":2,"exportedAt":"","name":"","description":"","kind":"","dshVersion":"","runtimeId":"","args":[],"env":[],"plugins":[],"excludedPlugins":[]}"#;
        std::fs::write(&path, newer).unwrap();
        let err = read_template_from(&path).unwrap_err();
        assert!(err.contains("不支持的模板格式版本"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn preview_reports_downloads_engine_verdicts_and_notes() {
        let root = temp_root("preview");
        // DSH 0.1.5 NOT installed on this root; runtime is system (present).
        let template = InstanceTemplateFile {
            format_version: TEMPLATE_FORMAT_VERSION,
            exported_at: "now".into(),
            name: "测试模板".into(),
            description: "d".into(),
            kind: "sandbox".into(),
            dsh_version: "0.1.5".into(),
            runtime_id: "node-system".into(),
            args: vec![],
            env: vec![
                TemplateEnvEntry {
                    key: "HTTP_PROXY".into(),
                    value: Some("http://p:1".into()),
                },
                TemplateEnvEntry {
                    key: "MY_API_KEY".into(),
                    value: None,
                },
            ],
            plugins: vec![
                TemplatePluginEntry {
                    plugin_id: "x/ok".into(),
                    registry_id: "dsh-ok".into(),
                    version: "1.0.0".into(),
                    enabled: true,
                    source: Some(crate::plugins::PluginSourceWire::Npm {
                        pkg: "dsh-ok".into(),
                    }),
                    source_status: "recorded".into(),
                    engine_range: Some("^0.1.0".into()),
                },
                TemplatePluginEntry {
                    plugin_id: "x/old".into(),
                    registry_id: "dsh-old".into(),
                    version: "0.9.0".into(),
                    enabled: true,
                    source: None,
                    source_status: "missing".into(),
                    engine_range: None,
                },
            ],
            excluded_plugins: vec![],
        };

        let preview = super::preview_template_inner(&root, &template)
            .await
            .unwrap();
        assert!(!preview.dsh_version_installed);
        assert!(preview.runtime_installed, "node-system counts as present");
        assert!(preview.blocked.is_empty());
        assert!(
            preview.notes.iter().any(|n| n.contains("DSH 0.1.5 未安装")),
            "notes: {:?}",
            preview.notes
        );
        let ok = &preview.plugins[0];
        assert_eq!(ok.engine_verdict, "compatible", "{}", ok.engine_reason);
        let old = &preview.plugins[1];
        assert_eq!(old.source_status, "missing");
        assert_eq!(old.engine_verdict, "unknown");
        assert!(preview.notes.iter().any(|n| n.contains("无法自动重装")));
        assert!(preview.notes.iter().any(|n| n.contains("重新配置值")));

        // Missing DSH version on the template → blocked, deterministically.
        let mut broken = template.clone();
        broken.dsh_version = String::new();
        let preview = super::preview_template_inner(&root, &broken).await.unwrap();
        assert!(preview
            .blocked
            .iter()
            .any(|b| b.contains("未绑定 DSH 版本")));
        let _ = std::fs::remove_dir_all(&root);
    }
}

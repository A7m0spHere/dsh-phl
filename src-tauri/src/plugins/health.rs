//! The plugin-environment health check (Issue #9).
//!
//! Read-only diagnosis for a PHL-managed instance's plugin layer: does what
//! `cordis.patch.yml` declares, what actually sits in `node_modules`, what
//! the PHL install markers record, and what the install transaction's
//! journal says — all agree? When they do not, the report names the finding,
//! its evidence, its impact and the suggestion; only the two repairs that go
//! through EXISTING safe entry points (transaction recovery and the
//! known-conflict disable) are offered as actionable, everything else is
//! advice the user executes (PHL does not delete `node_modules`, does not
//! treat unknown plugins or DSH bundles as un-installable PHL plugins, and
//! does not pretend to diagnose third-party runtime errors).
//!
//! External (in-place adopted) instances: the scan stays read-only like
//! everywhere else and the report flags `managed: false` — the UI hides the
//! repair buttons for them (PHL 写入边界，spec §3.4).

use std::path::Path;

use serde::Serialize;
use tauri::State;

use super::cordis::declared_plugin_entries_in_order;
use super::install::{recover_profile_transaction, transaction_residue};

/// The two repairs with a safe, existing entry point (issue #9: 自动建议
/// 必须有可靠证据、可回滚). Anything else is advice, not a button.
pub(crate) const REPAIR_RECOVER_TRANSACTION: &str = "recover-transaction";
pub(crate) const REPAIR_DISABLE_CONFLICTS: &str = "disable-conflicts";

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHealthFinding {
    /// Stable kind: `half-registered`（有 marker 目录但 cordis 未登记）、
    /// `declared-but-broken`（cordis 声明了但包缺失/损坏）、`orphan-dir`
    /// （node_modules 残留，既未登记也无 marker）、`version-mismatch`
    /// （marker 与 package.json 版本不一致）、`transaction-residue`、
    /// `known-conflict`。
    pub kind: String,
    /// `info` | `warning` | `error`.
    pub severity: String,
    /// The registry id the finding is about, or `-` when none applies.
    pub plugin: String,
    pub evidence: String,
    pub impact: String,
    pub suggestion: String,
    /// True only when a safe repair entry point exists for this finding.
    pub repairable: bool,
    /// The repair action id, present exactly when `repairable` is true.
    pub action: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHealthReport {
    pub instance_id: String,
    /// False for external (in-place adopted) instances: findings are still
    /// computed (the scan is read-only), but every repair is refused.
    pub managed: bool,
    pub generated_at: String,
    pub findings: Vec<PluginHealthFinding>,
}

/// The real package dirs in `node_modules` (top level plus one `@scope`
/// level; dotted entries are the installers' staging dirs and skipped).
async fn real_package_dirs(profile: &Path) -> Vec<String> {
    let node_modules = profile.join("node_modules");
    let mut out = Vec::new();
    let mut push_entries = |dir: &Path, prefix: Option<&str>| {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            match prefix {
                Some(scope) => out.push(format!("{scope}/{name}")),
                None => {
                    if name.starts_with('@') {
                        // The scope level itself is walked right after.
                        continue;
                    }
                    out.push(name);
                }
            }
        }
    };
    push_entries(&node_modules, None);
    // @scope level: enumerate scope dirs, then one level inside each.
    let Ok(scope_entries) = std::fs::read_dir(&node_modules) else {
        return out;
    };
    for entry in scope_entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('@') && path.is_dir() {
            push_entries(&path, Some(&name));
        }
    }
    out
}

/// One declared plugin's on-disk consistency. `marker_version` is the
/// version recorded in the PHL install marker when one exists.
async fn check_declared(
    profile: &Path,
    id: &str,
    marker_version: Option<&str>,
    scanned_by_phl: bool,
    findings: &mut Vec<PluginHealthFinding>,
) {
    let dir = profile.join("node_modules").join(id);
    if !dir.exists() {
        findings.push(PluginHealthFinding {
            kind: "declared-but-broken".into(),
            severity: "error".into(),
            plugin: id.into(),
            evidence: format!("cordis.patch.yml 声明了 {id}，但 node_modules 下没有该目录"),
            impact: "DSH 启动时会尝试加载一个不存在的插件，可能报错或被跳过".into(),
            suggestion: "重新安装该插件（走既有安装事务），或在插件页卸载清理登记".into(),
            repairable: false,
            action: None,
        });
        return;
    }
    if !dir.join("phl-plugin.json").exists() {
        if !scanned_by_phl {
            // cordis 登记了、目录在、但既没有 PHL marker，也没有任何已
            // 登记身份（bundle/profile package.json 都不认识它）——
            // 典型的「dsh plugin add 或手工放置」的外部安装，只提示。
            findings.push(PluginHealthFinding {
                kind: "half-registered".into(),
                severity: "info".into(),
                plugin: id.into(),
                evidence: "cordis.patch.yml 登记了该插件，但没有 PHL 安装记录（marker）".into(),
                impact: "PHL 的启停/卸载控制对它不可用；DSH 本身会正常加载".into(),
                suggestion: "如需 PHL 管理，在插件页重新安装一次；否则保持原样".into(),
                repairable: false,
                action: None,
            });
        }
        return;
    }
    // Marker exists → package.json must parse and its version must agree.
    let package_version = tokio::fs::read_to_string(dir.join("package.json"))
        .await
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("version").and_then(|v| v.as_str()).map(String::from));
    match package_version {
        None => findings.push(PluginHealthFinding {
            kind: "declared-but-broken".into(),
            severity: "error".into(),
            plugin: id.into(),
            evidence: "插件目录缺少可解析的 package.json".into(),
            impact: "DSH 无法加载该插件".into(),
            suggestion: "重新安装或卸载该插件".into(),
            repairable: false,
            action: None,
        }),
        Some(version) => {
            if marker_version != Some(version.as_str()) {
                findings.push(PluginHealthFinding {
                    kind: "version-mismatch".into(),
                    severity: "warning".into(),
                    plugin: id.into(),
                    evidence: format!(
                        "安装记录版本 {mv} ≠ 包内 package.json 版本 {v}",
                        mv = marker_version.unwrap_or("?"),
                        v = version
                    ),
                    impact: "包可能被外部改动过，或安装记录来自旧安装；行为可能与登记信息不符"
                        .into(),
                    suggestion: "重新安装该插件以重建一致的安装记录".into(),
                    repairable: false,
                    action: None,
                });
            }
        }
    }
}

/// Collect the findings for one profile. All IO is read-only; the function
/// never writes (asserted by `diagnosis_does_not_touch_the_disk`).
pub(crate) async fn collect_findings(profile: &Path) -> Vec<PluginHealthFinding> {
    let mut findings = Vec::new();

    // 1. Install-transaction residue (安装事务残留). A journal the recovery
    //    entry cannot read gets its own finding WITHOUT a repair button —
    //    offering one that is guaranteed to fail would be a lie.
    match transaction_residue(profile).await {
        super::install::TransactionResidue::Recoverable(id) => {
            findings.push(PluginHealthFinding {
                kind: "transaction-residue".into(),
                severity: "warning".into(),
                plugin: id,
                evidence: "profile 下存在未完成的 .phl-plugin-txn 安装事务日志".into(),
                impact:
                    "下一次安装/启停会先自动恢复该事务；忽略可能在磁盘上留下旧版本备份或半安装目录"
                        .into(),
                suggestion: "执行「恢复安装事务」（复用既有恢复入口，失败时保留旧版本、可重试）"
                    .into(),
                repairable: true,
                action: Some(REPAIR_RECOVER_TRANSACTION.into()),
            });
        }
        super::install::TransactionResidue::Unreadable => {
            findings.push(PluginHealthFinding {
                kind: "transaction-journal-unreadable".into(),
                severity: "warning".into(),
                plugin: "-".into(),
                evidence: "profile 下存在 .phl-plugin-txn 事务日志，但当前版本无法解析它".into(),
                impact: "可能来自更新版本的 PHL 或已损坏的 journal；自动恢复入口对它必然失败".into(),
                suggestion: "保留现场（不要手动删除 .phl-plugin-txn 及其中的备份目录），升级 PHL 后重新体检或人工处理"
                    .into(),
                repairable: false,
                action: None,
            });
        }
        super::install::TransactionResidue::None => {}
    }

    // 2. Known conflicts (已知会导致 WebUI 启动失败的组合).
    for conflict in super::conflicts::enabled_conflicts(profile).await {
        findings.push(PluginHealthFinding {
            kind: "known-conflict".into(),
            severity: "error".into(),
            plugin: conflict.ids.join(" + "),
            evidence: conflict.message,
            impact: "已知会导致 WebUI 启动失败或功能异常的插件组合".into(),
            suggestion: "执行「停用冲突插件」（复用 conflicts 的既有停用入口，可手动重新启用）"
                .into(),
            repairable: true,
            action: Some(REPAIR_DISABLE_CONFLICTS.into()),
        });
    }

    // 3. Per-declared-plugin consistency: cordis 声明 vs 磁盘 vs marker.
    let declared = declared_plugin_entries_in_order(profile).await;
    let scanned = crate::instances::scan_plugins(profile).await;
    for (id, _disabled) in &declared {
        let marker_version = scanned
            .iter()
            .find(|p| &p.registry_id == id)
            .map(|p| p.version.clone());
        let scanned_by_phl = scanned.iter().any(|p| &p.registry_id == id);
        check_declared(
            profile,
            id,
            marker_version.as_deref(),
            scanned_by_phl,
            &mut findings,
        )
        .await;
    }

    // 4. Half-registered the other way around: a PHL-marked install whose
    //    cordis entry never landed (a crash between marker write and
    //    registration, before any recovery ran). DSH only loads what cordis
    //    declares, so the copy exists but will not load.
    for p in &scanned {
        if !p.manageable || p.linked {
            continue;
        }
        if declared.iter().any(|(id, _)| id == &p.registry_id) {
            continue;
        }
        let dir = profile.join("node_modules").join(&p.registry_id);
        if !dir.join("phl-plugin.json").exists() {
            continue;
        }
        findings.push(PluginHealthFinding {
            kind: "half-registered".into(),
            severity: "warning".into(),
            plugin: p.registry_id.clone(),
            evidence: "插件目录带 PHL 安装记录，但 cordis.patch.yml 没有它的登记项".into(),
            impact: "DSH 只加载 cordis 登记的插件，该插件实际不会被加载".into(),
            suggestion: "重新安装该插件（走既有安装事务重建登记），或在插件页卸载清理".into(),
            repairable: false,
            action: None,
        });
    }

    // 5. Residue: dirs on disk that nothing registered (既未登记也无 marker，
    //    也不是 DSH bundle——bundle 由 scan_plugins 以 manageable=false 报告，
    //    在这里不重复上报)。
    let dirs = real_package_dirs(profile).await;
    for name in dirs {
        let declared_here = declared.iter().any(|(id, _)| id == &name);
        let scanned_here = scanned.iter().any(|p| p.registry_id == name);
        if !declared_here && !scanned_here {
            findings.push(PluginHealthFinding {
                kind: "orphan-dir".into(),
                severity: "info".into(),
                plugin: name,
                evidence: "node_modules 下存在未被登记也未带安装记录的目录".into(),
                impact: "通常是残留或手工放置；DSH 不会加载未登记的目录".into(),
                suggestion: "如不再需要可手动删除；PHL 不自动删除 node_modules（非目标）".into(),
                repairable: false,
                action: None,
            });
        }
    }

    findings
}

/// Command: read-only plugin-environment diagnosis for one instance. Works
/// on external instances too — reads are never gated; the report simply
/// flags `managed: false` and the UI hides the repair buttons.
#[tauri::command]
pub async fn plugin_health_check(
    phl: State<'_, crate::paths::PhlState>,
    instance_id: String,
) -> Result<PluginHealthReport, String> {
    let root = phl.root();
    let dir = crate::instances::instance_dir(&root, &instance_id)?;
    let manifest = crate::instances::manifest::load_manifest(&dir, &instance_id).await?;
    let profile = crate::instances::profile_dir(&root, &instance_id).await?;
    let findings = collect_findings(&profile).await;
    Ok(PluginHealthReport {
        instance_id,
        managed: manifest.management_mode != crate::instances::ManagementMode::External,
        generated_at: crate::versions::now_iso(),
        findings,
    })
}

/// Command: run the repairs the user explicitly chose (issue #9: 修复必须
/// 用户选择). Only the two safe entry points are accepted; anything else is
/// refused rather than improvised. Returns a FRESH health report so the UI
/// shows the post-repair truth.
#[tauri::command]
pub async fn plugin_health_repair(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, crate::paths::PhlState>,
    instance_id: String,
    actions: Vec<String>,
) -> Result<PluginHealthReport, String> {
    if actions.is_empty() {
        return Err("没有选择任何修复动作".into());
    }
    // External-instance write gate + the manifest the action needs.
    let profile =
        crate::instances::writable_profile_dir(&phl.root(), &instance_id, "修复插件环境于").await?;
    for action in &actions {
        match action.as_str() {
            REPAIR_RECOVER_TRANSACTION => {
                // The existing recovery entry: restores the backup, replays
                // the registration state, and cleans the journal — with its
                // own fail-closed semantics. Run under the shared lock.
                let profile = profile.clone();
                crate::resources::guarded(
                    crate::resources::next_task_id("plugin-health-repair"),
                    "plugin-health-repair",
                    "恢复插件安装事务".to_string(),
                    vec![crate::resources::Resource::Instance(instance_id.clone())],
                    None,
                    &locks,
                    &tasks,
                    move |_| async move { recover_profile_transaction(&profile).await },
                )
                .await?;
            }
            REPAIR_DISABLE_CONFLICTS => {
                let profile = profile.clone();
                crate::resources::guarded(
                    crate::resources::next_task_id("plugin-health-repair"),
                    "plugin-health-repair",
                    "停用已知冲突插件".to_string(),
                    vec![crate::resources::Resource::Instance(instance_id.clone())],
                    None,
                    &locks,
                    &tasks,
                    move |_| async move {
                        super::conflicts::disable_all(&profile).await.map(|_| ())
                    },
                )
                .await?;
            }
            other => {
                return Err(format!(
                    "未知的修复动作「{other}」——体检只提供有既有安全入口的两类修复，其余按建议人工处理"
                ));
            }
        }
    }
    // Report the post-repair truth, not the pre-repair hopes.
    let root = phl.root();
    let dir = crate::instances::instance_dir(&root, &instance_id)?;
    let manifest = crate::instances::manifest::load_manifest(&dir, &instance_id).await?;
    let profile_after = crate::instances::profile_dir(&root, &instance_id).await?;
    let findings = collect_findings(&profile_after).await;
    Ok(PluginHealthReport {
        instance_id,
        managed: manifest.management_mode != crate::instances::ManagementMode::External,
        generated_at: crate::versions::now_iso(),
        findings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_profile(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "phl-plugin-health-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        dir
    }

    /// A fully-consistent fixture plugin: dir + package.json + marker +
    /// cordis registration, built through the REAL primitives so the
    /// fixtures cannot drift from the formats those modules actually write.
    async fn install_fixture(profile: &Path, id: &str, version: &str) {
        let dir = profile.join("node_modules").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            format!(r#"{{"name":"{id}","version":"{version}"}}"#),
        )
        .unwrap();
        super::super::install::commit_install(
            &dir,
            &serde_json::json!({
                "installedAt": "now",
                "pluginId": format!("x/{id}"),
                "version": version,
                "kind": "npm",
                "registryId": id,
                "trust": "verified",
            }),
            profile,
            id,
            &crate::resources::Tasks::default()
                .begin(
                    crate::resources::TaskInfo::new(
                        "health-fixture".into(),
                        "plugin-install",
                        "t".into(),
                        &[],
                    ),
                    None,
                )
                .unwrap(),
            None,
            &super::super::deps::DependencyEnv::new(std::path::Path::new(""), "node-system", None),
        )
        .await
        .unwrap();
    }

    fn kinds(findings: &[PluginHealthFinding]) -> Vec<&str> {
        findings.iter().map(|f| f.kind.as_str()).collect()
    }

    fn dir_listing(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            out.push(path.to_string_lossy().into_owned());
            if path.is_dir() {
                dir_listing(&path, out);
            }
        }
        out.sort();
    }

    #[tokio::test]
    async fn clean_profile_has_no_findings() {
        let profile = temp_profile("clean");
        install_fixture(&profile, "dsh-foo", "1.0.0").await;
        let findings = collect_findings(&profile).await;
        assert_eq!(findings, Vec::new(), "{findings:?}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn diagnosis_does_not_touch_the_disk() {
        let profile = temp_profile("readonly");
        install_fixture(&profile, "dsh-foo", "1.0.0").await;
        // Deliberately broken state too: a residue dir the diagnosis must
        // not "helpfully" clean up (issue #9: 不自动删除 node_modules).
        std::fs::create_dir_all(profile.join("node_modules").join("dsh-residue")).unwrap();
        let mut before = Vec::new();
        dir_listing(&profile, &mut before);
        collect_findings(&profile).await;
        let mut after = Vec::new();
        dir_listing(&profile, &mut after);
        assert_eq!(before, after, "diagnosis must be read-only");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn half_registered_marker_without_cordis_is_flagged() {
        let profile = temp_profile("halfreg");
        // Marker + package.json land, but the cordis entry never did.
        let dir = profile.join("node_modules").join("dsh-foo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{"name":"dsh-foo","version":"1.0.0"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("phl-plugin.json"),
            r#"{"pluginId":"x/dsh-foo","registryId":"dsh-foo","version":"1.0.0","kind":"npm","trust":"verified"}"#,
        )
        .unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(kinds(&findings), vec!["half-registered"], "{findings:?}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn declared_but_missing_dir_is_an_error() {
        let profile = temp_profile("missing");
        super::super::cordis::register_cordis_patch(&profile, "dsh-ghost", None)
            .await
            .unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(
            kinds(&findings),
            vec!["declared-but-broken"],
            "{findings:?}"
        );
        assert_eq!(findings[0].severity, "error");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn version_mismatch_between_marker_and_package_is_flagged() {
        let profile = temp_profile("vmm");
        // Register with the real commit path, then hand-edit the package
        // version behind PHL's back — the classic "包被外部改动" shape.
        install_fixture(&profile, "dsh-foo", "1.0.0").await;
        std::fs::write(
            profile
                .join("node_modules")
                .join("dsh-foo")
                .join("package.json"),
            r#"{"name":"dsh-foo","version":"2.0.0"}"#,
        )
        .unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(kinds(&findings), vec!["version-mismatch"], "{findings:?}");
        assert!(findings[0].evidence.contains("1.0.0"));
        assert!(findings[0].evidence.contains("2.0.0"));
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn orphan_dir_is_info_and_never_auto_removed() {
        let profile = temp_profile("orphan");
        std::fs::create_dir_all(profile.join("node_modules").join("dsh-residue")).unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(kinds(&findings), vec!["orphan-dir"], "{findings:?}");
        assert_eq!(findings[0].severity, "info");
        assert!(!findings[0].repairable);
        assert!(profile.join("node_modules").join("dsh-residue").exists());
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn transaction_residue_is_detectable_and_recoverable() {
        let profile = temp_profile("txn");
        install_fixture(&profile, "dsh-foo", "1.0.0").await;
        // A stalled journal in the exact shape the installer writes.
        std::fs::create_dir_all(profile.join(".phl-plugin-txn")).unwrap();
        std::fs::write(
            profile.join(".phl-plugin-txn").join("journal.json"),
            r#"{"version":1,"registry_id":"dsh-foo","had_previous":false,"registration_before":null,"phase":"Prepared"}"#,
        )
        .unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(
            kinds(&findings),
            vec!["transaction-residue"],
            "{findings:?}"
        );
        assert!(findings[0].repairable);
        assert_eq!(
            findings[0].action.as_deref(),
            Some(REPAIR_RECOVER_TRANSACTION)
        );
        // The repair goes through the EXISTING recovery entry…
        recover_profile_transaction(&profile).await.unwrap();
        // …and the fresh diagnosis is clean again.
        let findings = collect_findings(&profile).await;
        assert_eq!(findings, Vec::new(), "{findings:?}");
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn an_unreadable_journal_is_reported_without_a_repair_button() {
        let profile = temp_profile("txnbad");
        std::fs::create_dir_all(profile.join(".phl-plugin-txn")).unwrap();
        // version != 1: the recovery entry refuses it by design, so the
        // health check must NOT offer the recover button (review R1).
        std::fs::write(
            profile.join(".phl-plugin-txn").join("journal.json"),
            r#"{"version":2,"registry_id":"dsh-foo","had_previous":false,"registration_before":null,"phase":"Prepared"}"#,
        )
        .unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(
            kinds(&findings),
            vec!["transaction-journal-unreadable"],
            "{findings:?}"
        );
        assert!(!findings[0].repairable);
        assert!(findings[0].action.is_none());
        let _ = std::fs::remove_dir_all(&profile);
    }

    #[tokio::test]
    async fn known_conflict_is_flagged_and_disable_all_repairs_it() {
        let profile = temp_profile("conflict");
        for id in ["browser-use", "browser-use-chrome-devtools-mcp"] {
            install_fixture(&profile, id, "1.0.0").await;
        }
        let findings = collect_findings(&profile).await;
        assert_eq!(kinds(&findings), vec!["known-conflict"], "{findings:?}");
        assert!(findings[0].repairable);
        assert_eq!(
            findings[0].action.as_deref(),
            Some(REPAIR_DISABLE_CONFLICTS)
        );
        // The existing conflict disable entry is the repair.
        super::super::conflicts::disable_all(&profile)
            .await
            .unwrap();
        let findings = collect_findings(&profile).await;
        assert_eq!(findings, Vec::new(), "{findings:?}");
        let _ = std::fs::remove_dir_all(&profile);
    }
}

//! Read-only environment facts and diffing (R1 · M2).
//!
//! `inspect_environment` collects what one instance's environment *is* —
//! bindings, the actually-installed DSH, the Node binary's real reported
//! version, plugin facts in bundle order, the API binding's shape (never its
//! secrets), the HOME/workspace relation and known plugin conflicts. Every
//! fact carries `known | unknown | unavailable` so a read failure can never
//! masquerade as "empty" or "same".
//!
//! `compare_environments` diffs two collected fact sets. It is strictly
//! read-only: no writes, no DSH dump invocation, no session-body parsing.
//! The fingerprint deliberately excludes collection time and anything
//! derived from volatile state, so collecting the same static environment
//! twice yields the same fingerprint.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;

use crate::instances::manifest::{InstanceManifest, ManagementMode};
use crate::paths::PhlState;

pub(crate) const FACTS_SCHEMA_VERSION: u32 = 1;

/* ------------------------------ wire types ----------------------------- */

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FactValue {
    /// `known` — read and meaningful; `unknown` — present but not decodable
    /// (e.g. a marker that names no version); `unavailable` — the source
    /// could not be read at all.
    pub state: String,
    pub value: Option<String>,
}

pub(crate) fn known(value: impl Into<String>) -> FactValue {
    FactValue {
        state: "known".into(),
        value: Some(value.into()),
    }
}

pub(crate) fn unknown() -> FactValue {
    FactValue {
        state: "unknown".into(),
        value: None,
    }
}

pub(crate) fn unavailable() -> FactValue {
    FactValue {
        state: "unavailable".into(),
        value: None,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshFacts {
    /// The binding as recorded in the manifest (`dsh-<ver>` shape).
    pub declared: FactValue,
    /// The version the installed tree's marker records.
    pub installed: FactValue,
    /// The npm integrity the install recorded — the stable "install
    /// fingerprint"; the marker itself carries volatile timestamps.
    pub integrity: FactValue,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeFacts {
    /// The binding as recorded in the manifest (`node-…` / `node-system`).
    pub binding: FactValue,
    /// What the bound binary actually reports via `node --version`.
    pub actual: FactValue,
    /// Recorded by precise installs; legacy markers carry none (unknown).
    pub platform: FactValue,
    pub arch: FactValue,
    /// True when the binding is the system PATH entry — an external, mutable
    /// resource by definition.
    pub system: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginFact {
    /// The real package identity (npm package name) plugins are matched by;
    /// same-name entries from different registries are never merged.
    pub id: String,
    /// The id used in `cordis.patch.yml` (may differ from the package name).
    pub registry_id: String,
    pub version: FactValue,
    pub trust: FactValue,
    pub enabled: Option<bool>,
    /// The insert-block order in `cordis.patch.yml` — bundle-stack order is
    /// semantics and must survive comparisons (B03).
    pub order: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiFacts {
    /// `default` | `custom` | `none` — the binding's shape, never its keys.
    pub inheritance: String,
    /// Provider ids the binding names; display only, no secret material.
    pub providers: Vec<String>,
    pub has_default_model: bool,
    /// True when the instance will launch without any provider configured.
    pub pending_credentials: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentFacts {
    pub schema_version: u32,
    pub instance_id: String,
    pub instance_name: String,
    pub management_mode: String,
    pub source: String,
    pub observed_at: String,
    /// Stable across repeated collections of a static environment; excludes
    /// observation time and volatile state. Not a byte-level snapshot claim.
    pub fingerprint: String,
    pub dsh: DshFacts,
    pub node: NodeFacts,
    pub profile: String,
    /// Empty when the whole profile could not be read — never silently "no
    /// plugins". `plugins_available: false` says why the list is empty.
    pub plugins_available: bool,
    pub plugins: Vec<PluginFact>,
    pub api: ApiFacts,
    /// Same string for both sides only when the two instances share one
    /// workspace path; values are local display, sharing reports placeholder
    /// them (M5).
    pub workspace: FactValue,
    /// Resolved agents home (`DSH_AGENTS_HOME` / `~/.agents` analogue): the
    /// residual shared boundary HOME isolation does not cover.
    pub agents_home_shared: bool,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffItem {
    /// Stable category key the frontend maps to a label.
    pub key: String,
    pub category: String,
    /// `same` | `different` | `left-only` | `right-only` | `unknown`
    pub state: String,
    pub left: Option<String>,
    pub right: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDiff {
    pub left_id: String,
    pub right_id: String,
    pub left_name: String,
    pub right_name: String,
    pub left_observed_at: String,
    pub right_observed_at: String,
    /// True when either side's facts hit an unavailable source — the report
    /// must say so instead of implying a clean comparison.
    pub has_unknown: bool,
    pub items: Vec<DiffItem>,
}

/* ------------------------------- collect ------------------------------- */

fn marker_string_field(dir: &Path, marker: &str, field: &str) -> FactValue {
    let Ok(raw) = std::fs::read_to_string(dir.join(marker)) else {
        return unavailable();
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) => match v.get(field).and_then(|f| f.as_str()) {
            Some(s) if !s.is_empty() => known(s),
            Some(_) => unknown(),
            None => unknown(),
        },
        Err(_) => unknown(),
    }
}

/// The bare version directory name behind a canonical `dsh-<ver>` binding.
fn bare_version(binding: &str) -> String {
    binding.strip_prefix("dsh-").unwrap_or(binding).to_string()
}

pub(crate) async fn collect_facts(
    root: &Path,
    dir: &Path,
    manifest: &InstanceManifest,
) -> EnvironmentFacts {
    let observed_at = crate::versions::now_iso();

    /* ---- DSH ---- */
    let declared = if manifest.version_id.trim().is_empty() {
        unknown()
    } else {
        known(&manifest.version_id)
    };
    let bare = bare_version(&manifest.version_id);
    let version_dir = root.join("versions").join(&bare);
    let dsh_installed = if bare.is_empty() {
        unknown()
    } else if !version_dir.exists() {
        unavailable()
    } else {
        marker_string_field(&version_dir, "phl-install.json", "version")
    };
    let dsh_integrity = if bare.is_empty() || !version_dir.exists() {
        unavailable()
    } else {
        marker_string_field(&version_dir, "phl-install.json", "integrity")
    };

    /* ---- Node ---- */
    let node_binding = known(&manifest.runtime_id);
    let (node_actual, node_platform, node_arch) = if manifest.runtime_id == "node-system" {
        // A08: the system entry is external and mutable, which is exactly
        // why its ACTUAL version must be observed — a silent PATH upgrade
        // is a real environment change. One bounded process launch.
        match crate::runtimes::probe_system_node_version().await {
            Some(v) => (known(v), known("system"), known("system")),
            None => (unknown(), known("system"), known("system")),
        }
    } else {
        let rt_dir = root.join("runtimes").join(&manifest.runtime_id);
        if !rt_dir.exists() {
            (unavailable(), unavailable(), unavailable())
        } else {
            let marker = crate::runtimes::runtime_marker(&rt_dir);
            let platform = marker
                .as_ref()
                .and_then(|m| m.platform.clone())
                .map(known)
                .unwrap_or_else(unknown);
            let arch = marker
                .as_ref()
                .and_then(|m| m.arch.clone())
                .map(known)
                .unwrap_or_else(unknown);
            let bin = crate::launch::runtime_bin_dir(root, &manifest.runtime_id)
                .join(crate::launch::node_binary());
            let actual = match crate::runtimes::probe_node_version(&bin).await {
                Some(v) => known(v),
                None => {
                    if bin.exists() {
                        unknown() // binary exists but would not report a version
                    } else {
                        unavailable()
                    }
                }
            };
            (actual, platform, arch)
        }
    };

    /* ---- plugins ---- */
    // External instances keep their profile inside their own DSH_HOME —
    // deriving the path from the instance dir would silently read nothing
    // (CR-10).
    let profile_dir = crate::instances::profile_root_of(dir, manifest);
    let (plugins, plugins_available) = if !profile_dir.exists() {
        (Vec::new(), false)
    } else {
        // Bundle-stack order: the profile's rebuilt package.json
        // (`dsh.profile.bundles`) is the loader's authoritative stack when
        // present; the cordis patch's insert order backs it up for
        // instances that never came from a pack.
        let mut order_index: BTreeMap<String, usize> = BTreeMap::new();
        if let Ok(pkg_raw) = tokio::fs::read_to_string(profile_dir.join("package.json")).await {
            if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&pkg_raw) {
                if let Some(bundles) = pkg
                    .pointer("/dsh/profile/bundles")
                    .and_then(|b| b.as_array())
                {
                    for (index, b) in bundles.iter().enumerate() {
                        if let Some(name) = b.as_str() {
                            order_index.entry(name.to_string()).or_insert(index);
                        }
                    }
                }
            }
        }
        let order = crate::plugins::cordis::declared_plugin_entries_in_order(&profile_dir).await;
        for (index, (id, _)) in order.iter().enumerate() {
            order_index.entry(id.clone()).or_insert(index);
        }
        // A directory that exists but cannot be read is NOT an empty plugin
        // list — surface it as unavailable (CR-10/B04).
        match tokio::fs::read_dir(profile_dir.join("node_modules")).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => (Vec::new(), false),
            _ => {
                let scanned = crate::instances::scan_plugins(&profile_dir).await;
                // Sort by bundle order where the stack knows it; entries the
                // stack does not mention keep a stable id order after them.
                let mut facts: Vec<PluginFact> = scanned
                    .into_iter()
                    .map(|p| PluginFact {
                        order: order_index.get(&p.plugin_id).copied().unwrap_or(usize::MAX),
                        id: p.plugin_id.clone(),
                        registry_id: p.registry_id.clone(),
                        version: if p.version.trim().is_empty() {
                            unknown()
                        } else {
                            known(&p.version)
                        },
                        trust: if p.trust == "unknown" {
                            unknown()
                        } else {
                            known(&p.trust)
                        },
                        enabled: Some(p.enabled),
                    })
                    .collect();
                facts.sort_by(|a, b| {
                    a.order
                        .cmp(&b.order)
                        .then_with(|| a.registry_id.cmp(&b.registry_id))
                });
                (facts, true)
            }
        }
    };

    /* ---- API binding shape ---- */
    let api_binding = manifest.api.clone().unwrap_or_default();
    let pending_credentials = api_binding.inheritance == "none"
        || (api_binding.provider_ids.is_empty() && api_binding.inheritance != "custom");
    let api = ApiFacts {
        inheritance: api_binding.inheritance,
        providers: api_binding.provider_ids,
        has_default_model: api_binding.default_model.is_some(),
        pending_credentials,
    };

    /* ---- paths & agents home ---- */
    let workspace_dir = dir.join("workspace");
    let workspace =
        if workspace_dir.exists() || manifest.management_mode == ManagementMode::ManagedCopy {
            known(workspace_dir.to_string_lossy().into_owned())
        } else {
            unknown()
        };
    // PHL does not set DSH_AGENTS_HOME today, so every instance shares the
    // user's global agents home — recorded as the residual shared boundary
    // it is. (Trial copies flip this by setting the env var per instance.)
    let agents_home_shared = !manifest.env.contains_key("DSH_AGENTS_HOME");

    /* ---- known plugin conflicts ---- */
    let conflicts = crate::plugins::conflicts::enabled_conflicts(&profile_dir)
        .await
        .into_iter()
        .map(|c| c.message)
        .collect();

    let mut facts = EnvironmentFacts {
        schema_version: FACTS_SCHEMA_VERSION,
        instance_id: manifest.id.clone(),
        instance_name: manifest.name.clone(),
        management_mode: manifest.management_mode.as_str().into(),
        source: serde_json::to_value(manifest.source)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
        observed_at,
        fingerprint: String::new(),
        dsh: DshFacts {
            declared,
            installed: dsh_installed,
            integrity: dsh_integrity,
        },
        node: NodeFacts {
            binding: node_binding,
            actual: node_actual,
            platform: node_platform,
            arch: node_arch,
            system: manifest.runtime_id == "node-system",
        },
        profile: manifest.profile.clone(),
        plugins_available,
        plugins,
        api,
        workspace,
        agents_home_shared,
        conflicts,
    };
    facts.fingerprint = fingerprint(&facts);
    facts
}

/// Stable hash over the environment's meaningful facts. Deliberately
/// excludes `observed_at`, paths that only matter locally, and secret
/// material (there is none here by construction).
fn fingerprint(facts: &EnvironmentFacts) -> String {
    let mut hasher = Sha256::new();
    // Order the feeding fields explicitly: the wire struct's serialization
    // is for humans, the fingerprint must not drift with field reordering.
    for part in [
        facts.dsh.declared.value.as_deref().unwrap_or("∅"),
        facts.dsh.installed.state.as_str(),
        facts.dsh.installed.value.as_deref().unwrap_or("∅"),
        facts.dsh.integrity.value.as_deref().unwrap_or("∅"),
        facts.node.binding.value.as_deref().unwrap_or("∅"),
        facts.node.actual.value.as_deref().unwrap_or("∅"),
        facts.node.platform.value.as_deref().unwrap_or("∅"),
        facts.node.arch.value.as_deref().unwrap_or("∅"),
        &facts.profile,
        &facts.api.inheritance,
        &facts.api.providers.join(","),
        facts.workspace.value.as_deref().unwrap_or("∅"),
        if facts.agents_home_shared {
            "shared"
        } else {
            "isolated"
        },
    ] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    for p in &facts.plugins {
        for part in [
            &p.registry_id,
            p.version.value.as_deref().unwrap_or("∅"),
            p.trust.value.as_deref().unwrap_or("∅"),
            &p.enabled.map(|b| b.to_string()).unwrap_or_default(),
            &p.order.to_string(),
        ] {
            hasher.update(part.as_bytes());
            hasher.update([0]);
        }
    }
    for c in &facts.conflicts {
        hasher.update(c.as_bytes());
        hasher.update([0]);
    }
    hex::encode(hasher.finalize())
}

/* -------------------------------- diff --------------------------------- */

fn diff_pair(left: &FactValue, right: &FactValue) -> (String, Option<String>, Option<String>) {
    // Any side that could not be read or decoded stays unknown — never a
    // false "same", never a phantom value.
    if left.state != "known" || right.state != "known" {
        return ("unknown".into(), value_of(left), value_of(right));
    }
    let state = if left.value == right.value {
        "same"
    } else {
        "different"
    };
    (state.into(), left.value.clone(), right.value.clone())
}

fn value_of(v: &FactValue) -> Option<String> {
    match v.state.as_str() {
        "known" => v.value.clone(),
        "unknown" => Some("未知".into()),
        _ => Some("不可读取".into()),
    }
}

fn item(
    key: &str,
    category: &str,
    state: String,
    left: Option<String>,
    right: Option<String>,
    note: Option<String>,
) -> DiffItem {
    DiffItem {
        key: key.into(),
        category: category.into(),
        state,
        left,
        right,
        note,
    }
}

/// Diffs two collected fact sets. Both sides are already snapshots; the
/// comparison itself touches nothing on disk.
pub(crate) fn build_diff(left: &EnvironmentFacts, right: &EnvironmentFacts) -> EnvironmentDiff {
    let mut items = Vec::new();

    // DSH: declared binding and the actually installed build.
    let (state, l, r) = diff_pair(&left.dsh.declared, &right.dsh.declared);
    items.push(item("dsh.binding", "DSH", state, l, r, None));
    let (state, l, r) = diff_pair(&left.dsh.installed, &right.dsh.installed);
    items.push(item("dsh.installed", "DSH", state, l, r, None));
    // Same declared binding but different install fingerprint → someone
    // reinstalled one side; worth its own row rather than a false "same".
    let integrity_state =
        if left.dsh.integrity.state == "known" && right.dsh.integrity.state == "known" {
            if left.dsh.integrity.value == right.dsh.integrity.value {
                "same"
            } else {
                "different"
            }
        } else {
            "unknown"
        };
    items.push(item(
        "dsh.integrity",
        "DSH",
        integrity_state.into(),
        value_of(&left.dsh.integrity),
        value_of(&right.dsh.integrity),
        Some("安装指纹（npm integrity）".into()),
    ));

    let (state, l, r) = diff_pair(&left.node.binding, &right.node.binding);
    items.push(item("node.binding", "Node", state, l, r, None));
    let (state, l, r) = diff_pair(&left.node.actual, &right.node.actual);
    items.push(item("node.actual", "Node", state, l, r, None));

    let (state, l, r) = if left.profile == right.profile {
        (
            "same".into(),
            Some(left.profile.clone()),
            Some(right.profile.clone()),
        )
    } else {
        (
            "different".into(),
            Some(left.profile.clone()),
            Some(right.profile.clone()),
        )
    };
    items.push(item("profile", "Profile", state, l, r, None));

    /* ---- plugins: identity-keyed left/right merge ---- */
    let left_by_id: BTreeMap<&str, &PluginFact> =
        left.plugins.iter().map(|p| (p.id.as_str(), p)).collect();
    let right_by_id: BTreeMap<&str, &PluginFact> =
        right.plugins.iter().map(|p| (p.id.as_str(), p)).collect();

    if !left.plugins_available || !right.plugins_available {
        items.push(item(
            "plugins.scan",
            "插件",
            "unknown".into(),
            (!left.plugins_available).then(|| "插件目录不可读取".into()),
            (!right.plugins_available).then(|| "插件目录不可读取".into()),
            Some("无法读取的一侧按未知呈现，不视作无插件".into()),
        ));
    }

    let mut ids: Vec<&str> = left_by_id
        .keys()
        .chain(right_by_id.keys())
        .copied()
        .collect();
    ids.sort_unstable();
    ids.dedup();
    for id in &ids {
        match (left_by_id.get(*id), right_by_id.get(*id)) {
            (Some(l), Some(r)) => {
                let version_differs =
                    l.version.value != r.version.value || l.version.state != r.version.state;
                let enabled_differs = l.enabled != r.enabled;
                let order_differs = l.order != r.order;
                if version_differs || enabled_differs || order_differs {
                    let mut notes: Vec<String> = Vec::new();
                    if version_differs {
                        notes.push("版本".into());
                    }
                    if enabled_differs {
                        notes.push("启用状态".into());
                    }
                    if order_differs {
                        notes.push("bundle 顺序".into());
                    }
                    items.push(item(
                        &format!("plugin.{id}"),
                        "插件",
                        "different".into(),
                        Some(plugin_display(l)),
                        Some(plugin_display(r)),
                        Some(notes.join("、")),
                    ));
                }
            }
            (Some(l), None) => {
                items.push(item(
                    &format!("plugin.{id}"),
                    "插件",
                    "left-only".into(),
                    Some(plugin_display(l)),
                    None,
                    None,
                ));
            }
            (None, Some(r)) => {
                items.push(item(
                    &format!("plugin.{id}"),
                    "插件",
                    "right-only".into(),
                    None,
                    Some(plugin_display(r)),
                    None,
                ));
            }
            (None, None) => unreachable!(),
        }
    }

    /* ---- API binding shape ---- */
    let api_same = left.api.inheritance == right.api.inheritance
        && left.api.providers == right.api.providers
        && left.api.has_default_model == right.api.has_default_model;
    items.push(item(
        "api",
        "API 配置来源",
        if api_same {
            "same".into()
        } else {
            "different".into()
        },
        Some(api_display(&left.api)),
        Some(api_display(&right.api)),
        None,
    ));

    /* ---- workspace relation ---- */
    let (state, l, r) = diff_pair(&left.workspace, &right.workspace);
    let workspace_note = (state == "different").then(|| {
        "两个实例的工作目录不同；若其中一侧是共享目录，双方会操作同一批项目文件".to_string()
    });
    items.push(item("workspace", "工作目录", state, l, r, workspace_note));

    let agents_state = if left.agents_home_shared == right.agents_home_shared {
        "same"
    } else {
        "different"
    };
    items.push(item(
        "agents-home",
        "共享边界",
        agents_state.into(),
        Some(agents_display(left.agents_home_shared)),
        Some(agents_display(right.agents_home_shared)),
        Some("DSH 的 skill 扫描可能读取全局 agents 目录（DSH_AGENTS_HOME / ~/.agents）".into()),
    ));

    let conflicts_same = left.conflicts == right.conflicts;
    items.push(item(
        "conflicts",
        "已知冲突",
        if conflicts_same {
            "same".into()
        } else {
            "different".into()
        },
        Some(left.conflicts.join("；")),
        Some(right.conflicts.join("；")),
        None,
    ));

    // Stable ordering: category first, then key — the frontend renders rows
    // in this order, so repeated comparisons never reshuffle.
    items.sort_by(|a, b| a.category.cmp(&b.category).then_with(|| a.key.cmp(&b.key)));

    let has_unknown = items.iter().any(|i| i.state == "unknown");
    EnvironmentDiff {
        left_id: left.instance_id.clone(),
        right_id: right.instance_id.clone(),
        left_name: left.instance_name.clone(),
        right_name: right.instance_name.clone(),
        left_observed_at: left.observed_at.clone(),
        right_observed_at: right.observed_at.clone(),
        has_unknown,
        items,
    }
}

fn plugin_display(p: &PluginFact) -> String {
    let version = p
        .version
        .value
        .clone()
        .unwrap_or_else(|| p.version.state.clone());
    let enabled = match p.enabled {
        Some(true) => "启用",
        Some(false) => "停用",
        None => "未知",
    };
    format!("{version} · {enabled} · 顺序 {}", {
        if p.order == usize::MAX {
            "—".to_string()
        } else {
            p.order.to_string()
        }
    })
}

fn api_display(api: &ApiFacts) -> String {
    let mode = match api.inheritance.as_str() {
        "default" => "继承全部",
        "custom" => "自定义绑定",
        "none" => "不托管",
        other => other,
    };
    if api.providers.is_empty() {
        format!("{mode}（无供应商）")
    } else {
        format!("{mode}：{} 个供应商", api.providers.len())
    }
}

fn agents_display(shared: bool) -> String {
    if shared {
        "共享全局 agents 目录".into()
    } else {
        "实例隔离（DSH_AGENTS_HOME）".into()
    }
}

/* ------------------------------- commands ------------------------------ */

async fn load_instance(
    root: &Path,
    id: &str,
) -> Result<(std::path::PathBuf, InstanceManifest), String> {
    let dir = crate::instances::instance_dir(root, id)?;
    let manifest = crate::instances::read_manifest(&dir)
        .await
        .ok_or_else(|| "实例不存在或清单不可读".to_string())?;
    Ok((dir, manifest))
}

#[tauri::command]
pub async fn inspect_environment(
    phl: State<'_, PhlState>,
    instance_id: String,
) -> Result<EnvironmentFacts, String> {
    let root = phl.root().to_path_buf();
    let (dir, manifest) = load_instance(&root, &instance_id).await?;
    Ok(collect_facts(&root, &dir, &manifest).await)
}

#[tauri::command]
pub async fn compare_environments(
    phl: State<'_, PhlState>,
    left_id: String,
    right_id: String,
) -> Result<EnvironmentDiff, String> {
    let root = phl.root().to_path_buf();
    let (left_dir, left_manifest) = load_instance(&root, &left_id).await?;
    let (right_dir, right_manifest) = load_instance(&root, &right_id).await?;
    let left = collect_facts(&root, &left_dir, &left_manifest).await;
    let right = collect_facts(&root, &right_dir, &right_manifest).await;
    Ok(build_diff(&left, &right))
}

/* -------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests;

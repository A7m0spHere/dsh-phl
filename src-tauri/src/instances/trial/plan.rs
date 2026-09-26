//! Trial plan identity and its persisted record (CR-08, R2-03, R3-02).
//!
//! The preview issues a plan; the plan covers the request's own fields, the
//! source fingerprint and the copy's **allocated port**. That last field is
//! why the plan is written to disk: the moment the copy commits, its port is
//! a configured port like any other, so re-deriving the plan from live state
//! produced a *different* plan id for the same plan — and a retry after a
//! lost response was refused as stale (R3-02).
//!
//! So create resolves the plan in two steps, in this order:
//!
//! 1. **Committed copy of this plan** — read from the copy's own persisted
//!    records (`instance.json` + `phl-trial.json`). Nothing is allocated
//!    here: the copy's port being registered or already listening is its own
//!    footprint, never a reason for its plan to expire.
//! 2. **Persisted plan record** — version, runtime, port and fingerprint come
//!    from what the preview issued, not from a fresh allocation. A plan that
//!    was never issued (or whose record is gone) is refused rather than
//!    rebuilt from live state.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::TrialRequest;
use crate::instances::{instance_dir, instances_root, is_external, load_manifest};
use crate::paths::sanitize_segment;

/* ---------------------------- plan identity ---------------------------- */

/// One issued plan, exactly as the preview computed it. `plan_id` is the hash
/// the frontend sends back; every other field is what create re-verifies
/// before it copies anything.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrialPlan {
    pub plan_id: String,
    pub source_id: String,
    pub target_id: String,
    pub target_version: String,
    pub target_runtime_id: String,
    pub scope: String,
    pub workspace: String,
    pub source_fingerprint: String,
    pub allocated_port: u16,
    pub auto_port: bool,
    pub created_at: String,
}

impl TrialPlan {
    /// The plan a preview just issued, as it is persisted. The preview's
    /// normalized version/runtime are what a create must ask for, so they are
    /// recorded rather than the raw request fields.
    pub(crate) fn from_preview(preview: &super::TrialPreview) -> Self {
        Self {
            plan_id: preview.plan_id.clone(),
            source_id: preview.source_id.clone(),
            target_id: preview.target_id.clone(),
            target_version: preview.target_version.clone(),
            target_runtime_id: preview.target_runtime_id.clone(),
            scope: preview.scope.clone(),
            workspace: preview.workspace.clone(),
            source_fingerprint: preview.source_fingerprint.clone(),
            allocated_port: preview.allocated_port,
            auto_port: preview.auto_port,
            created_at: crate::versions::now_iso(),
        }
    }
}

/// The tuple `create_trial` needs to run the copy: (id, name, version, bound
/// version id, runtime, port, auto-port).
pub(crate) type ValidatedPlan = (String, String, String, String, String, u16, bool);

/// Where the create goes: either a committed copy of this plan (report it) or
/// a fresh plan to execute. The committed arm carries only what the retry
/// needs to READ — the copy's directory — because nothing about it is
/// recomputed; the copy's own manifest is the description.
pub(crate) enum PlanRoute {
    Fresh(ValidatedPlan),
    Committed { dest: PathBuf, id: String },
}

/* -------------------------- plan persistence --------------------------- */

/// Issued plans, kept beside the data root so a retry survives an app
/// restart. They are single-use records that exist to serve exactly one
/// late/duplicate request, so old ones are swept on write.
fn plans_root(root: &Path) -> PathBuf {
    root.join("trial-plans")
}

const PLAN_RETENTION_MS: u128 = 7 * 24 * 60 * 60 * 1000;

pub(crate) async fn write_trial_plan(root: &Path, plan: &TrialPlan) -> Result<(), String> {
    let dir = plans_root(root);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("无法记录试升级计划: {e}"))?;
    let id = sanitize_segment(&plan.plan_id, "计划 id")?;
    let bytes = serde_json::to_vec_pretty(plan).map_err(|e| e.to_string())?;
    // Atomic replace: a torn plan file would refuse a create the preview just
    // issued, which is the one thing this record exists to prevent.
    let tmp = dir.join(format!(".{id}.tmp"));
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| format!("无法记录试升级计划: {e}"))?;
    tokio::fs::rename(&tmp, dir.join(format!("{id}.json")))
        .await
        .map_err(|e| format!("无法记录试升级计划: {e}"))?;
    prune_trial_plans(&dir);
    Ok(())
}

pub(crate) async fn load_trial_plan(root: &Path, plan_id: &str) -> Option<TrialPlan> {
    let id = sanitize_segment(plan_id, "计划 id").ok()?;
    let raw = tokio::fs::read_to_string(plans_root(root).join(format!("{id}.json")))
        .await
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// Best-effort sweep of expired plan records. A failure here must never fail
/// the preview that issued the newest plan.
fn prune_trial_plans(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age.as_millis() > PLAN_RETENTION_MS);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/* ------------------------- fingerprints and ids ------------------------ */

/// Stable fingerprint of the source environment a plan was built from:
/// manifest bytes, the bound version's install marker, the profile's patch +
/// rebuilt machine file, the plugin scan (id/version/enabled) and — for the
/// sessions scope — the session count. Any change to any of these invalidates
/// outstanding plans (C16).
pub(crate) async fn source_fingerprint(
    root: &Path,
    source_dir: &Path,
    manifest: &crate::instances::InstanceManifest,
    scope: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut feed = |bytes: &[u8]| hasher.update(bytes);
    if let Ok(raw) = tokio::fs::read_to_string(source_dir.join("instance.json")).await {
        feed(raw.as_bytes());
    }
    let bare = manifest
        .version_id
        .strip_prefix("dsh-")
        .unwrap_or(&manifest.version_id)
        .to_string();
    if let Ok(raw) =
        tokio::fs::read_to_string(root.join("versions").join(&bare).join("phl-install.json")).await
    {
        feed(raw.as_bytes());
    }

    // Profile facts: patch text, machine file, plugin scan.
    let profile = crate::instances::profile_root(source_dir, &manifest.profile);
    for rel in ["cordis.patch.yml", "package.json"] {
        match tokio::fs::read(profile.join(rel)).await {
            Ok(bytes) => {
                feed(&bytes);
            }
            Err(_) => feed(b"<absent>"),
        }
    }
    let mut plugin_summary: Vec<String> = crate::instances::scan_plugins(&profile)
        .await
        .into_iter()
        .map(|p| format!("{}|{}|{}", p.plugin_id, p.version, p.enabled))
        .collect();
    plugin_summary.sort();
    feed(plugin_summary.join("\n").as_bytes());

    // Sessions scope: the count the copy would carry is part of the plan.
    if scope_wants_sessions(scope) {
        let home = crate::instances::home_of(source_dir, manifest);
        let count =
            tokio::task::spawn_blocking(move || crate::sessions::list_in_home(&home).0.len())
                .await
                .unwrap_or(0);
        feed(count.to_string().as_bytes());
    }

    hex::encode(hasher.finalize())
}

/// The plan id both preview and create derive from every field the copy
/// depends on, including the port the plan allocated. Create recomputes it
/// from the PERSISTED plan plus the fresh fingerprint — a mismatch means the
/// request's fields or the source drifted (C16).
#[allow(clippy::too_many_arguments)]
pub(crate) fn derive_plan_id(
    source_id: &str,
    target_version: &str,
    target_runtime_id: &str,
    scope: &str,
    workspace: &str,
    target_id: &str,
    allocated_port: u16,
    auto_port: bool,
    fingerprint: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for part in [
        source_id,
        target_version,
        target_runtime_id,
        scope,
        workspace,
        target_id,
        fingerprint,
        &allocated_port.to_string(),
        if auto_port { "auto" } else { "fixed" },
    ] {
        h.update(part.as_bytes());
        h.update([0]);
    }
    hex::encode(h.finalize())
}

pub(crate) fn scope_wants_sessions(scope: &str) -> bool {
    scope == "config+sessions"
}

/// The target runtime id as the plan records it: `node-system` verbatim, any
/// other id sanitized like every other path/argv segment.
pub(crate) fn normalize_runtime_id(raw: &str) -> Result<String, String> {
    if raw == "node-system" {
        Ok("node-system".to_string())
    } else {
        sanitize_segment(raw, "Runtime id")
    }
}

/// Whether a DSH version's install marker exists — the same signal
/// `pack::install` and the version page read.
pub(crate) async fn version_installed(root: &Path, bare: &str) -> bool {
    tokio::fs::read_to_string(root.join("versions").join(bare).join("phl-install.json"))
        .await
        .is_ok()
}

/* ------------------------------ validation ----------------------------- */

/// Resolves the request to a route: a committed copy of this plan, or a fresh
/// plan to execute. Everything a create depends on is checked here, from the
/// plan's OWN records (CR-08/C16/R3-02).
pub(crate) async fn plan_route(
    root: &Path,
    processes: &crate::launch::Processes,
    req: &TrialRequest,
) -> Result<PlanRoute, String> {
    // The plan gates everything: a create without a preview-issued plan would
    // skip the source-fingerprint re-check and the plan identity check
    // (CR-08/C16).
    if req.plan_id.trim().is_empty() || req.target_id.trim().is_empty() {
        return Err("缺少试升级计划：请先完成预览再创建副本".into());
    }
    let source_dir = instance_dir(root, &req.source_id)?;
    let source_manifest = load_manifest(&source_dir, &req.source_id).await?;
    if is_external(&source_manifest) {
        return Err("原地接入的实例不支持试升级：请先「复制到 PHL」转为受管实例".into());
    }
    let target_version = crate::versions::install::sanitize_version(&req.target_version)?;
    let target_runtime_id = normalize_runtime_id(&req.target_runtime_id)?;
    let new_id = sanitize_segment(&req.target_id, "实例 id")?;

    // 1. A committed copy of THIS plan answers first (R3-02), from its own
    // persisted records — no allocation, no fingerprint re-derivation, so a
    // running copy cannot invalidate its own plan.
    if let Some((dest, id)) =
        committed_copy_of_plan(root, req, &target_version, &target_runtime_id, &new_id).await?
    {
        return Ok(PlanRoute::Committed { dest, id });
    }

    // 2. Otherwise the persisted plan is the identity authority.
    let plan = load_trial_plan(root, &req.plan_id)
        .await
        .ok_or_else(|| "试升级计划不存在或已过期：请重新预览后再创建".to_string())?;
    if plan.plan_id != req.plan_id
        || plan.source_id != req.source_id
        || plan.target_id != new_id
        || plan.target_version != target_version
        || plan.target_runtime_id != target_runtime_id
        || plan.scope != req.scope
        || plan.workspace != req.workspace
    {
        return Err("计划与当前选项不一致：请重新预览后再创建".into());
    }
    if !version_installed(root, &target_version).await {
        return Err(format!(
            "目标 DSH 版本 {target_version} 未安装，请先在版本页安装"
        ));
    }
    let target_version_id =
        crate::versions::install::bound_id_for_version(&target_version).unwrap_or_default();
    if target_runtime_id != "node-system" {
        let marker = root
            .join("runtimes")
            .join(&target_runtime_id)
            .join("phl-runtime.json");
        if !tokio::fs::try_exists(&marker).await.unwrap_or(false) {
            return Err(format!(
                "目标 Runtime {target_runtime_id} 未安装，请先在 Runtime 页安装"
            ));
        }
    }

    // The source must still be what the preview saw (CR-08/C16). The plan's
    // own fingerprint is compared too, so a request cannot claim one
    // fingerprint while its plan recorded another.
    let fresh_fingerprint =
        source_fingerprint(root, &source_dir, &source_manifest, &req.scope).await;
    if fresh_fingerprint != req.source_fingerprint || fresh_fingerprint != plan.source_fingerprint {
        return Err("源实例在预览后发生了变化：计划已过期，请重新预览".into());
    }
    // The plan id must still derive from the persisted plan's own port: the
    // request cannot smuggle another plan's identity, and the check no longer
    // depends on a fresh port allocation (R3-02).
    let recomputed = derive_plan_id(
        &req.source_id,
        &target_version,
        &target_runtime_id,
        &req.scope,
        &req.workspace,
        &new_id,
        plan.allocated_port,
        plan.auto_port,
        &fresh_fingerprint,
    );
    if recomputed != req.plan_id {
        return Err("计划与当前选项不一致：请重新预览后再创建".into());
    }
    // The copy's port must still be unclaimed by anyone ELSE: a plan that has
    // not committed can be overtaken by another plan's copy, and refusing with
    // "re-preview" beats creating an instance that can never run beside it
    // (CR-07/C14). Deliberately NOT a bind probe: the plan's port is a
    // decision made at preview time, and a live bind test would make the
    // create depend on transient machine state — including its own copy's
    // registration once it has committed, which is exactly what made the
    // retry expire (R3-02).
    let claimed_by_instance = configured_fixed_ports(root, &new_id)
        .await
        .contains(&plan.allocated_port);
    let claimed_by_process = {
        let map = processes.0.lock().expect("processes lock");
        map.iter()
            .any(|(id, e)| e.port == plan.allocated_port && id.as_str() != new_id)
    };
    if claimed_by_instance || claimed_by_process {
        return Err(format!(
            "计划分配的端口 {} 已被其他实例占用：请重新预览生成新计划",
            plan.allocated_port
        ));
    }
    let name = if req.name.trim().is_empty() {
        format!("{} · 新版测试", source_manifest.name)
    } else {
        req.name.trim().to_string()
    };
    Ok(PlanRoute::Fresh((
        new_id,
        name,
        target_version,
        target_version_id,
        target_runtime_id,
        plan.allocated_port,
        plan.auto_port,
    )))
}

/// A copy of this very plan already on disk? Then the create is a retry, and
/// the copy's persisted records decide whether the request still describes it
/// (R3-02). A directory that is *not* this plan's copy refuses: one plan can
/// never overwrite another's instance.
async fn committed_copy_of_plan(
    root: &Path,
    req: &TrialRequest,
    target_version: &str,
    target_runtime_id: &str,
    new_id: &str,
) -> Result<Option<(PathBuf, String)>, String> {
    let dest = instance_dir(root, new_id)?;
    if !tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        return Ok(None);
    }
    let existing = load_manifest(&dest, new_id)
        .await
        .map_err(|e| format!("实例目录 {new_id} 已存在且清单不可读（{e}），拒绝覆盖"))?;
    let same_source_copy = existing
        .adopted_from
        .as_ref()
        .is_some_and(|a| a.dsh_home == format!("trial:{}", req.source_id));
    let marker = super::marker::read_trial_marker(&dest).await;
    let same_plan = same_source_copy && marker.as_ref().is_some_and(|m| m.plan_id == req.plan_id);
    if !same_plan {
        return Err(if same_source_copy {
            format!("实例目录 {new_id} 属于另一个试升级计划，拒绝覆盖；请重新预览生成新计划")
        } else {
            format!("实例目录已存在: {new_id}")
        });
    }
    let marker = marker.expect("same_plan requires a marker");

    // What was committed must be what this request asks for: version, runtime,
    // port and the plan's knobs all come from the persisted records. A
    // mismatch is a different plan wearing this plan's id — refuse rather
    // than report someone else's copy as this plan's outcome.
    let bound = crate::versions::install::bound_id_for_version(target_version).unwrap_or_default();
    let port_matches =
        marker.allocated_port == 0 || u32::from(marker.allocated_port) == existing.port;
    let identity_matches = !bound.is_empty()
        && existing.version_id == bound
        && existing.runtime_id == target_runtime_id
        && port_matches
        && (marker.target_version.is_empty() || marker.target_version == target_version)
        && (marker.scope.is_empty() || marker.scope == req.scope)
        && (marker.workspace.is_empty() || marker.workspace == req.workspace);
    if !identity_matches {
        return Err(format!(
            "实例目录 {new_id} 的已提交副本与当前计划参数不一致，拒绝复用；请重新预览生成新计划"
        ));
    }
    Ok(Some((dest, new_id.to_string())))
}

/// The copy's own port at plan time (CR-07/R2-03): avoids every instance's
/// configured fixed port (a stopped source still owns its binding), PHL's
/// running processes and a live OS bind. Used by the preview only — create
/// reads the port from the persisted plan.
pub(crate) async fn allocate_copy_port(
    root: &Path,
    processes: &crate::launch::Processes,
    source_port: u32,
    exclude_id: &str,
) -> Result<u16, String> {
    let configured_ports = configured_fixed_ports(root, exclude_id).await;
    for attempt in 1..=100u16 {
        let candidate = source_port.max(1024) as u16 + attempt;
        if configured_ports.contains(&candidate) {
            continue;
        }
        if let Ok(port) = crate::launch::allocate_port(processes, exclude_id, candidate, false) {
            return Ok(port);
        }
    }
    Err("无法为副本分配独立端口：请释放 1024 以上的端口后重试".into())
}

/// Every instance's configured fixed port (auto-port instances excluded —
/// they re-scan at launch), ignoring `exclude_id` so a plan can ask whether
/// anyone OTHER than its own target claims a port.
async fn configured_fixed_ports(root: &Path, exclude_id: &str) -> std::collections::HashSet<u16> {
    let mut out = std::collections::HashSet::new();
    let Ok(mut entries) = tokio::fs::read_dir(instances_root(root)).await else {
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_dir() || crate::paths::is_hidden_tree_name(&name) || name == exclude_id {
            continue;
        }
        if let Ok(manifest) = load_manifest(&path, &name).await {
            if !manifest.auto_port {
                out.insert(u16::try_from(manifest.port).unwrap_or(0));
            }
        }
    }
    out
}

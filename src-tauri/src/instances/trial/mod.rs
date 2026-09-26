//! Copy & trial upgrade (R1 · M3): duplicate a managed instance as an
//! independent copy bound to a *different* DSH version, so a new release can
//! be tried without touching the daily environment.
//!
//! The flow is preview → create. The preview is read-only and computes what
//! would be copied, what would have to download (it does not download), which
//! sessions would travel and which workspace paths they reference. The
//! create runs under the same resource locks the rest of the app uses, with
//! the source stopped: the stop check happens *inside* the lock so the
//! "check → copy → commit" window cannot race a launch.
//!
//! Copy scope is path-semantic at the tree root: `环境配置` drops sessions,
//! attachments, logs, snapshots and caches; `环境＋会话` keeps `sessions/`
//! but still drops the rest. A nested `logs/` inside some package is part of
//! that package, not run state, and is never dropped (same rule as clone).
//!
//! Links into the shared `versions/<old>/` tree are retargeted to
//! `versions/<new>/` after the copy, and every retarget is verified: a link
//! that cannot resolve against the new version is reported, never silently
//! left pointing at the old tree while the UI claims success.

mod marker;
mod plan;

pub(crate) use marker::{
    read_trial_marker, recorded_outcome_with, write_trial_marker, TrialMarker,
};
pub(crate) use plan::{
    allocate_copy_port, derive_plan_id, plan_route, scope_wants_sessions, source_fingerprint,
    version_installed, write_trial_plan, PlanRoute, TrialPlan,
};

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use super::copy::{copy_tree_with_progress, LinkPolicy, SkipRule};
use super::manifest::InstanceManifest;
use super::{build_record, instance_dir, instances_root, is_external, load_manifest, profile_root};
use crate::paths::{ensure_under_root, sanitize_segment, PhlState};

/* ------------------------------- requests ------------------------------ */

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialRequest {
    pub source_id: String,
    /// Bare DSH version to bind the copy to (e.g. `0.1.7-rc.2`). Always
    /// explicit: the flow never falls back to "latest".
    pub target_version: String,
    /// Precise runtime id (or `node-system`).
    pub target_runtime_id: String,
    /// Copy name; empty → the `源名称 · 新版测试` default.
    #[serde(default)]
    pub name: String,
    /// `config` | `config+sessions`
    #[serde(default = "default_scope")]
    pub scope: String,
    /// `fresh` (new empty workspace) | `shared` (junction to the source's)
    #[serde(default = "default_workspace")]
    pub workspace: String,
    /// The plan these fields execute — issued by the preview. Empty →
    /// refused: executing without a plan would skip the source-fingerprint
    /// re-check (C16).
    #[serde(default)]
    pub plan_id: String,
    #[serde(default)]
    pub target_id: String,
    #[serde(default)]
    pub source_fingerprint: String,
}

fn default_scope() -> String {
    "config".into()
}
fn default_workspace() -> String {
    "fresh".into()
}

/* ------------------------------- preview ------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialPreview {
    pub source_id: String,
    pub source_name: String,
    pub source_version_id: String,
    pub source_runtime_id: String,
    pub target_version: String,
    pub target_installed: bool,
    pub target_runtime_id: String,
    pub target_runtime_installed: bool,
    /// `源名称 · 新版测试`; the UI lets the user change it.
    pub suggested_name: String,
    pub scope: String,
    pub workspace: String,
    /// Bytes the copy would carry: dsh-home minus the skipped root entries,
    /// plus sessions for the wider scope. An estimate — free-space checks
    /// happen against the real copy.
    pub estimated_bytes: u64,
    /// Session count for `config+sessions`; `None` for `config`.
    pub session_count: Option<usize>,
    /// Distinct cwds the sessions reference — copying never rewrites them,
    /// and the preview must say so (M3 §9.2).
    pub session_cwds: Vec<String>,
    /// What the backend would need prepared before the copy (the frontend
    /// runs those through the normal install flows; create refuses if
    /// anything is still missing).
    pub pending_downloads: Vec<String>,
    pub conflicts: Vec<String>,
    /// Backend-issued plan identity (CR-08): create re-verifies the plan and
    /// the source fingerprint, and reuses the same target id so a retry can
    /// never create a second copy (C13/C11).
    pub plan_id: String,
    pub target_id: String,
    pub source_fingerprint: String,
    /// The copy's own port (CR-07): a fixed-port source does NOT hand its
    /// port to the copy — both instances must be able to run at once.
    pub allocated_port: u16,
    pub auto_port: bool,
    /// Non-empty → create refuses; each entry is a user-readable reason.
    pub blocked: Vec<String>,
}

/// Stable fingerprint of the source environment a plan was built from, and
/// the plan id both preview and create derive from every field the copy
/// depends on — both live in `plan`, with the rest of the plan identity.
pub(crate) async fn build_preview(
    root: &Path,
    processes: &crate::launch::Processes,
    req: &TrialRequest,
) -> Result<TrialPreview, String> {
    let source_dir = instance_dir(root, &req.source_id)?;
    let manifest = load_manifest(&source_dir, &req.source_id).await?;
    if is_external(&manifest) {
        // Not an error — a *preview* the UI can act on (offer adoption).
        let mut preview =
            preview_shell(root, &crate::launch::Processes::default(), req, &manifest).await?;
        preview.blocked.push(
            "原地接入的实例没有受管副本可以复制：请先通过「接入本机 DSH → 复制到 PHL」转为受管实例"
                .into(),
        );
        return Ok(preview);
    }

    let mut preview = preview_shell(root, processes, req, &manifest).await?;
    // Issue the plan: the record is what a later create re-verifies against,
    // including the copy's port, so a retry never has to re-derive it from
    // live state (R3-02). Only executable plans are recorded — a blocked
    // preview issues nothing.
    if preview.blocked.is_empty() {
        write_trial_plan(root, &TrialPlan::from_preview(&preview)).await?;
    }

    // Sessions for the wider scope: count + distinct cwds, headers only —
    // session bodies are never parsed for a preview.
    if scope_wants_sessions(&req.scope) {
        let home = super::home_of(&source_dir, &manifest);
        let sessions = tokio::task::spawn_blocking(move || crate::sessions::list_in_home(&home).0)
            .await
            .map_err(|e| e.to_string())?;
        let cwds: BTreeSet<String> = sessions
            .iter()
            .filter_map(|s| s.cwd.clone())
            .filter(|c| !c.trim().is_empty())
            .collect();
        preview.session_count = Some(sessions.len());
        preview.session_cwds = cwds.into_iter().collect();
    }

    // Estimated bytes: dsh-home minus what the copy skips.
    let source_home = super::home_of(&source_dir, &manifest);
    if source_home.exists() {
        preview.estimated_bytes = tokio::task::spawn_blocking({
            let source_home = source_home.clone();
            let skip_sessions = !scope_wants_sessions(&req.scope);
            move || estimate_bytes(&source_home, skip_sessions)
        })
        .await
        .map_err(|e| e.to_string())?;
    }

    Ok(preview)
}

async fn preview_shell(
    root: &Path,
    processes: &crate::launch::Processes,
    req: &TrialRequest,
    manifest: &InstanceManifest,
) -> Result<TrialPreview, String> {
    let target_version = crate::versions::install::sanitize_version(&req.target_version)?;
    // The same normalization create applies, so the recorded plan and the
    // re-verified request can never disagree about the runtime id.
    let target_runtime_id = plan::normalize_runtime_id(&req.target_runtime_id)?;

    let target_installed = version_installed(root, &target_version).await;
    let target_runtime_installed = if target_runtime_id == "node-system" {
        true
    } else {
        tokio::fs::read_to_string(
            root.join("runtimes")
                .join(&target_runtime_id)
                .join("phl-runtime.json"),
        )
        .await
        .is_ok()
    };

    let mut pending = Vec::new();
    if !target_installed {
        pending.push(format!("DSH {target_version} 未安装（先通过版本页安装）"));
    }
    if !target_runtime_installed {
        pending.push(format!(
            "Runtime {target_runtime_id} 未安装（先通过 Runtime 页安装）"
        ));
    }

    let conflicts = crate::plugins::conflicts::enabled_conflicts(&profile_root(
        &instance_dir(root, &req.source_id)?,
        &manifest.profile,
    ))
    .await
    .into_iter()
    .map(|c| c.message)
    .collect();

    let source_version = manifest.version_id.clone();
    // CR-07: the copy gets its own port at plan time, with the SAME
    // allocation rule create uses — otherwise the recomputed plan id could
    // never match (R2-03).
    let auto_port = manifest.auto_port;
    let allocated_port =
        allocate_copy_port(root, processes, manifest.port, "trial-preview").await?;
    let source_dir = instance_dir(root, &req.source_id)?;
    let fingerprint = source_fingerprint(root, &source_dir, manifest, &req.scope).await;
    // The plan owns a target id up front so a retried create lands on the
    // same directory instead of spawning a second copy (C13/C11). The id is
    // time-suffixed so two previews can never claim the same target.
    let target_id = format!(
        "trial-{}-{}",
        sanitize_segment(&req.source_id, "实例 id")?,
        crate::versions::now_millis()
    );
    let plan_id = derive_plan_id(
        &req.source_id,
        &target_version,
        &target_runtime_id,
        &req.scope,
        &req.workspace,
        &target_id,
        allocated_port,
        auto_port,
        &fingerprint,
    );

    Ok(TrialPreview {
        source_id: req.source_id.clone(),
        source_name: manifest.name.clone(),
        source_version_id: source_version,
        source_runtime_id: manifest.runtime_id.clone(),
        target_version,
        target_installed,
        target_runtime_id,
        target_runtime_installed,
        suggested_name: format!("{} · 新版测试", manifest.name),
        scope: req.scope.clone(),
        workspace: req.workspace.clone(),
        estimated_bytes: 0,
        session_count: None,
        session_cwds: Vec::new(),
        pending_downloads: pending,
        conflicts,
        plan_id,
        target_id,
        source_fingerprint: fingerprint,
        allocated_port,
        auto_port,
        blocked: Vec::new(),
    })
}

/// Root-level entries a trial copy leaves behind. Path-semantic by design:
/// the skip list applies to the tree root only, never by basename.
fn trial_skips(name: &str, skip_sessions: bool) -> bool {
    if super::copy::skipped(name) {
        return true; // logs / snapshots / .phl-cache / .phl-*
    }
    match name {
        // Session data and their attachments are content the user opted into
        // separately; a config-only copy never carries them.
        "sessions" => skip_sessions,
        "attachments" => true,
        // The workspace dir is recreated per strategy (empty, or a junction
        // to the source's when the user chose shared).
        "workspace" => true,
        _ => false,
    }
}

fn estimate_bytes(source_home: &Path, skip_sessions: bool) -> u64 {
    let Ok(entries) = std::fs::read_dir(source_home) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if trial_skips(&name, skip_sessions) {
            continue;
        }
        total += super::copy::dir_size(&entry.path());
    }
    total
}

/* -------------------------------- create ------------------------------- */

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialOutcome {
    pub record: crate::instances::InstanceRecord,
    /// True when the instance was registered. Commit is the rename: after it
    /// a cancel/interrupt leaves a *created* copy behind, and the UI must say
    /// so instead of reporting a pure cancel.
    pub committed: bool,
    /// `needsDependencies | needsCredentials | readyToLaunch` — registration
    /// alone is not "the environment works".
    pub readiness: String,
    /// Link retargets into `versions/<new>` that verified.
    pub link_redirects: usize,
    /// Retargets that could not resolve against the new version — each entry
    /// names the link and why. Non-empty → readiness degrades.
    pub link_failures: Vec<String>,
    pub sessions_imported: usize,
    pub notes: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn preview_trial(
    phl: State<'_, PhlState>,
    processes: State<'_, crate::launch::Processes>,
    req: TrialRequest,
) -> Result<TrialPreview, String> {
    build_preview(&phl.root(), &processes, &req).await
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn create_trial(
    transfers: State<'_, crate::versions::Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, crate::launch::Processes>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    req: TrialRequest,
    on_progress: Channel<super::CloneProgress>,
) -> Result<TrialOutcome, String> {
    create_trial_inner(
        &phl.root(),
        &transfers,
        &locks,
        &tasks,
        &processes,
        transfer_id,
        req,
        on_progress,
    )
    .await
}

/// The command's body, split out so tests drive the REAL entry — plan
/// validation, the committed-plan retry, the resource locks and the journal
/// all included (R3-02).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_trial_inner(
    root: &Path,
    transfers: &crate::versions::Transfers,
    locks: &crate::resources::ResourceLocks,
    tasks: &crate::resources::Tasks,
    processes: &crate::launch::Processes,
    transfer_id: String,
    req: TrialRequest,
    on_progress: Channel<super::CloneProgress>,
) -> Result<TrialOutcome, String> {
    let root = root.to_path_buf();
    let (
        new_id,
        name,
        target_version,
        target_version_id,
        target_runtime_id,
        allocated_port,
        auto_port,
    ) = match plan_route(&root, processes, &req).await? {
        // A committed copy of this plan answers immediately (R3-02): the
        // retry is a READ of what was created, so it takes no lock, writes no
        // journal row and allocates nothing.
        PlanRoute::Committed { dest, id } => return marker::recorded_outcome(&dest, &id).await,
        PlanRoute::Fresh(plan) => plan,
    };

    let flag = transfers.take(&transfer_id);
    // Journal span: the record is written before the copy starts and closed
    // on every exit path, so a crash leaves an honest `running` row that the
    // next boot marks interrupted.
    let mut detail = serde_json::Map::new();
    detail.insert(
        "dshVersion".into(),
        serde_json::Value::String(target_version.clone()),
    );
    detail.insert(
        "runtimeId".into(),
        serde_json::Value::String(target_runtime_id.clone()),
    );
    detail.insert("scope".into(), serde_json::Value::String(req.scope.clone()));
    detail.insert(
        "workspace".into(),
        serde_json::Value::String(req.workspace.clone()),
    );
    // The operation id derives from the plan: a retried create appends to
    // the same operation's journal thread instead of appearing as a second
    // operation.
    detail.insert(
        "planId".into(),
        serde_json::Value::String(req.plan_id.clone()),
    );
    let span_result = crate::operations::begin_trial(
        &root,
        &req.source_id,
        &new_id,
        &format!("复制并试用新版 → {name}"),
        detail,
    );
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "trial-create",
        format!("创建试升级副本 {name}"),
        vec![
            crate::resources::Resource::Instance(req.source_id.clone()),
            crate::resources::Resource::Instance(new_id.clone()),
            crate::resources::Resource::Version(target_version.clone()),
            // CR-09: the runtime the copy will bind is part of the plan's
            // footprint — a concurrent removal must wait, not interleave.
            crate::resources::Resource::Runtime(target_runtime_id.clone()),
        ],
        Some(flag.clone()),
        locks,
        tasks,
        |task| {
            let req = req.clone();
            async move {
                run_trial(
                    &flag,
                    &root,
                    &task,
                    processes,
                    &req,
                    &new_id,
                    &name,
                    &target_version,
                    &target_version_id,
                    &target_runtime_id,
                    allocated_port,
                    auto_port,
                    &on_progress,
                )
                .await
            }
        },
    )
    .await;
    transfers.release(&transfer_id);
    match &result {
        Ok(outcome) => {
            if let Ok(span) = span_result {
                let mut detail = serde_json::Map::new();
                detail.insert(
                    "dshVersion".into(),
                    serde_json::Value::String(outcome.record.manifest.version_id.clone()),
                );
                detail.insert(
                    "readiness".into(),
                    serde_json::Value::String(outcome.readiness.clone()),
                );
                detail.insert(
                    "linkRedirects".into(),
                    serde_json::json!(outcome.link_redirects),
                );
                detail.insert(
                    "sessionsImported".into(),
                    serde_json::json!(outcome.sessions_imported),
                );
                span.finish("committed", None, detail);
            }
        }
        Err(e) => {
            if let Ok(span) = span_result {
                span.finish("failed", Some(e.clone()), Default::default());
            }
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_trial(
    flag: &Arc<AtomicBool>,
    root: &Path,
    task: &crate::resources::Task,
    processes: &crate::launch::Processes,
    req: &TrialRequest,
    new_id: &str,
    name: &str,
    target_version: &str,
    target_version_id: &str,
    target_runtime_id: &str,
    allocated_port: u16,
    auto_port: bool,
    on_progress: &Channel<super::CloneProgress>,
) -> Result<TrialOutcome, String> {
    // The version/runtime guards live here, not only in the command: the
    // copy must never start against a target that is not on disk yet.
    if !version_installed(root, target_version).await {
        return Err(format!(
            "目标 DSH 版本 {target_version} 未安装，请先在版本页安装"
        ));
    }
    if target_runtime_id != "node-system" {
        let marker = root
            .join("runtimes")
            .join(target_runtime_id)
            .join("phl-runtime.json");
        if !tokio::fs::try_exists(&marker).await.unwrap_or(false) {
            return Err(format!(
                "目标 Runtime {target_runtime_id} 未安装，请先在 Runtime 页安装"
            ));
        }
    }

    let source_dir = instance_dir(root, &req.source_id)?;
    let source_manifest = load_manifest(&source_dir, &req.source_id).await?;
    // The authoritative stop check: inside the same lock the launcher takes,
    // so a launch raced against this window is impossible.
    super::snapshot::ensure_not_running(processes, &req.source_id)?;
    // Lock-window fingerprint re-check (R2-03): the plan's source state is
    // revalidated now that the source is exclusively ours.
    let fresh = source_fingerprint(root, &source_dir, &source_manifest, &req.scope).await;
    if fresh != req.source_fingerprint {
        return Err("源实例在预览后发生了变化：计划已过期，请重新预览".into());
    }
    // C07/R2-04: the process table only knows about PHL-launched children.
    // A read-only recency check over the HOME catches an outside writer —
    // the source tree is never moved or mutated to find out.
    let source_home = super::home_of(&source_dir, &source_manifest);
    if source_home.exists() {
        assert_home_quiet(&source_home).await?;
    }

    let dest = instance_dir(root, new_id)?;
    if dest.exists() {
        // Idempotent retry (C13/C11): a committed copy of the SAME plan
        // already exists — report its RECORDED state instead of creating a
        // second instance or papering over a failed one (R2-03).
        if let Ok(existing) = load_manifest(&dest, new_id).await {
            let marker = read_trial_marker(&dest).await;
            let is_same_plan = existing
                .adopted_from
                .as_ref()
                .is_some_and(|a| a.dsh_home == format!("trial:{}", req.source_id))
                && marker.as_ref().is_some_and(|m| m.plan_id == req.plan_id);
            if is_same_plan {
                return Ok(recorded_outcome_with(&dest, existing).await);
            }
            return Err(format!(
                "实例目录 {new_id} 属于另一个试升级计划，拒绝覆盖；请重新预览生成新计划"
            ));
        }
        return Err(format!("实例目录已存在: {new_id}"));
    }
    let staging = instances_root(root).join(format!(".phl-trial-{new_id}"));
    // Idempotent retry: a leftover staging tree from a failed attempt is
    // this operation's own, boundary-checked name — removing it is safe.
    let _ = tokio::fs::remove_dir_all(&staging).await;

    task.set_phase("copying");
    let source_version_bare = source_manifest
        .version_id
        .strip_prefix("dsh-")
        .unwrap_or(&source_manifest.version_id)
        .to_string();

    let copy_result = copy_tree_with_progress(
        source_dir.clone(),
        staging.clone(),
        Arc::clone(flag),
        SkipRule::Trial {
            skip_sessions: !scope_wants_sessions(&req.scope),
            depth: 0,
        },
        LinkPolicy::Preserve {
            source_root: source_dir.clone(),
            dest_root: dest.clone(),
        },
        root.to_path_buf(),
        &|progress| {
            let _ = on_progress.send(progress);
        },
    )
    .await;
    if let Err(e) = copy_result {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }

    // Workspace strategy: fresh = empty dir (launch creates it), shared =
    // junction to the source's workspace so both instances operate on the
    // same project files. The preview said which one was chosen.
    if req.workspace == "shared" {
        let shared = source_dir.join("workspace");
        if shared.exists() {
            task.set_phase("configuring");
            if let Err(e) = super::copy::recreate_link(&staging.join("workspace"), &shared, true) {
                let _ = tokio::fs::remove_dir_all(&staging).await;
                return Err(format!("无法共享工作目录: {e}"));
            }
        }
    }

    // Retarget links that point into versions/<old> onto versions/<new>.
    task.set_phase("configuring");
    let staging_home = staging.join("dsh-home");
    let (redirects, link_failures) =
        retarget_version_links(&staging_home, root, &source_version_bare, target_version).await;

    // The copy's manifest: source environment, new bindings.
    let mut manifest = source_manifest.clone();
    super::canonicalize_version_binding(&mut manifest);
    manifest.id = new_id.to_string();
    manifest.name = name.to_string();
    manifest.version_id = target_version_id.to_string();
    manifest.runtime_id = target_runtime_id.to_string();
    // CR-07: a fixed-port source never hands its port to the copy. The
    // plan allocated a free one; an auto-port source keeps auto allocation.
    manifest.port = u32::from(allocated_port);
    manifest.auto_port = auto_port;
    manifest.created_at = crate::versions::now_iso();
    manifest.last_run_at = None;
    manifest.total_runtime = 0;
    manifest.favorite = false;
    manifest.source = super::manifest::InstanceSource::Created;
    manifest.adopted_from = Some(super::manifest::AdoptedFrom {
        dsh_home: format!("trial:{}", req.source_id),
        detected_version: Some(target_version.to_string()),
        adopted_at: crate::versions::now_iso(),
        mode: "trial-copy".into(),
    });
    // Isolate the residual shared boundary M0 verified: DSH's skill scan
    // reads the *global* agents home unless DSH_AGENTS_HOME says otherwise.
    // The copy keeps the user's daily `~/.agents` out of the experiment.
    manifest.env.insert(
        "DSH_AGENTS_HOME".to_string(),
        // The *final* path, not staging: the directory is renamed after this
        // write, and an env var pointing at the staging name would dangle.
        dest.join("dsh-home")
            .join(".agents")
            .to_string_lossy()
            .into_owned(),
    );

    // Sessions: count what actually travelled for the honest outcome.
    let sessions_imported = if scope_wants_sessions(&req.scope) {
        let dir = staging_home.join("sessions");
        count_session_dirs(&dir)
    } else {
        0
    };

    task.set_phase("committing");
    if let Err(e) = super::manifest::write_manifest(&staging, &manifest).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }
    tokio::fs::rename(&staging, &dest)
        .await
        .map_err(|e| format!("无法放置实例目录: {e}"))?;

    // COMMIT POINT — the instance now exists. Everything below reports on
    // what was created; cancelling past here must not delete it.
    let readiness = readiness_of(&manifest, &link_failures);
    // Record the plan identity + the true commit-time outcome: the idempotent
    // retry reports THIS, not a fabricated ready (R2-03), and re-verifies the
    // copy against the same version/runtime/port the plan asked for (R3-02).
    if let Err(e) = write_trial_marker(
        &dest,
        &TrialMarker {
            plan_id: req.plan_id.clone(),
            target_version: target_version.to_string(),
            target_runtime_id: target_runtime_id.to_string(),
            scope: req.scope.clone(),
            workspace: req.workspace.clone(),
            allocated_port,
            readiness: readiness.clone(),
            link_failures: link_failures.clone(),
        },
    )
    .await
    {
        #[cfg(test)]
        eprintln!("MARKER-WRITE-FAILED {e}");
        let _ = e;
    }
    if crate::versions::cancelled(flag) {
        let notes = vec!["副本已创建，后续检查已取消".to_string()];
        return Ok(TrialOutcome {
            record: build_record(&dest, manifest).await,
            committed: true,
            readiness,
            link_redirects: redirects,
            link_failures,
            sessions_imported,
            notes,
        });
    }

    super::invalidate_disk_usage(&dest);
    let mut notes = Vec::new();
    if !link_failures.is_empty() {
        notes.push(format!(
            "{} 个内部链接在新版本下无法解析，见结果详情；这些插件可能需要重新安装依赖",
            link_failures.len()
        ));
    }
    if scope_wants_sessions(&req.scope) && !preview_cwds_warned(&source_dir).await {
        notes.push("会话保留其原工作目录引用；副本不会自动执行任何任务".into());
    }
    Ok(TrialOutcome {
        record: build_record(&dest, manifest).await,
        committed: true,
        readiness,
        link_redirects: redirects,
        link_failures,
        sessions_imported,
        notes,
    })
}

fn readiness_of(manifest: &InstanceManifest, link_failures: &[String]) -> String {
    if !link_failures.is_empty() {
        return "needsDependencies".into();
    }
    // A `none`-inheritance binding means the user owns credentials; nothing
    // to verify without launching — reported as needsCredentials only when
    // PHL manages credentials and none are bound.
    match &manifest.api {
        Some(api) if api.inheritance == "none" => "needsCredentials".into(),
        _ => "readyToLaunch".into(),
    }
}

async fn preview_cwds_warned(_source_dir: &Path) -> bool {
    // The preview already listed the cwds; the outcome note is static.
    false
}

fn count_session_dirs(sessions_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return 0;
    };
    let mut count = 0;
    for project in entries.flatten() {
        if !project.path().is_dir() {
            continue;
        }
        if let Ok(sessions) = std::fs::read_dir(project.path()) {
            count += sessions.flatten().filter(|s| s.path().is_dir()).count();
        }
    }
    count
}

/// Retargets directory links under `staging_home` that point into the shared
/// `versions/<old>/` tree onto `versions/<new>/`, preserving the rest of the
/// path, and verifies every retargeted link resolves. Returns (redirected,
/// failures): a failure names a link that now does not resolve — the honest
/// signal that a plugin needs its dependencies rebuilt, not something to
/// hide behind a green result.
async fn retarget_version_links(
    home: &Path,
    root: &Path,
    old_bare: &str,
    new_bare: &str,
) -> (usize, Vec<String>) {
    if old_bare == new_bare {
        return (0, Vec::new());
    }
    let mut redirects = 0usize;
    let mut failures = Vec::new();
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let Ok(meta) = tokio::fs::symlink_metadata(&path).await else {
                continue;
            };
            // A reparse point covers both real symlinks and Windows
            // junctions; the copy engine lays both down as the latter.
            #[cfg(windows)]
            let is_reparse = {
                use std::os::windows::fs::MetadataExt;
                (meta.file_attributes() & 0x400) != 0 // FILE_ATTRIBUTE_REPARSE_POINT
            };
            #[cfg(not(windows))]
            let is_reparse = meta.file_type().is_symlink();
            if is_reparse {
                // Candidate: read where it points.
                if let Some(target) = read_link_target(&path) {
                    let versions_prefix = root.join("versions").join(old_bare);
                    if target.starts_with(&versions_prefix) {
                        let new_target = root.join("versions").join(new_bare).join(
                            target
                                .strip_prefix(&versions_prefix)
                                .unwrap_or(Path::new("")),
                        );
                        if new_target.exists() {
                            // A junction is a directory reparse point:
                            // remove_dir drops the link, not the target.
                            let _ = tokio::fs::remove_dir(&path).await;
                            let _ = tokio::fs::remove_file(&path).await;
                            match super::copy::recreate_link(&path, &new_target, true) {
                                Ok(()) => redirects += 1,
                                Err(e) => failures.push(format!(
                                    "{}: 重建失败 {e}",
                                    path.strip_prefix(home).unwrap_or(&path).display()
                                )),
                            }
                        } else {
                            failures.push(format!(
                                "{}: 新版本下不存在 {}",
                                path.strip_prefix(home).unwrap_or(&path).display(),
                                new_target.display()
                            ));
                        }
                    }
                }
                continue; // never descend into a reparse point
            }
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    (redirects, failures)
}

/// Where a link points, as an absolute path (junctions resolve through
/// canonicalize; symlink_metadata + read_link covers the rest).
fn read_link_target(path: &Path) -> Option<PathBuf> {
    if let Ok(target) = std::fs::read_link(path) {
        if target.is_absolute() {
            return Some(target);
        }
        if let Some(parent) = path.parent() {
            return Some(parent.join(target));
        }
        return None;
    }
    // A Windows junction is a directory reparse point; read_link covers it,
    // but canonicalize is the fallback that resolves the true location.
    std::fs::canonicalize(path).ok()
}

/// Keep `ensure_under_root` imported for future staging sweeps; the trial's
/// staging name is generated locally and boundary-checked by construction.
#[allow(dead_code)]
fn staging_boundary(root: &Path, staging: &Path) -> Result<(), String> {
    ensure_under_root(&instances_root(root), staging)
}

/// C07/R2-04: a READ-ONLY quiet check. The old probe renamed the source
/// HOME aside and back — a crash between the two renames left the daily
/// environment moved, which is exactly what a copy must never do. Rename
/// success also proved nothing about outside handles.
///
/// Signal: a live DSH writes its log continuously under `<home>/logs`
/// (M0 verified upstream). Fresh log writes ⇒ likely running ⇒ refuse.
/// Config files written by an install that just finished are not evidence
/// of a live process and do not trigger this.
const QUIET_WINDOW_MS: u128 = 120_000;

async fn assert_home_quiet(home: &Path) -> Result<(), String> {
    let logs_dir = home.join("logs");
    let Ok(mut entries) = tokio::fs::read_dir(&logs_dir).await else {
        return Ok(()); // no logs dir: no live-writer signal to check
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        if let Ok(modified) = meta.modified() {
            let age_ms = std::time::SystemTime::now()
                .duration_since(modified)
                .map(|d| d.as_millis())
                .unwrap_or(u128::MAX);
            if age_ms < QUIET_WINDOW_MS {
                return Err(format!(
                    "源 HOME 的日志最近 {} 秒内有写入（{}），疑似仍在运行；\
                     请确认实例与相关进程已全部停止后重试",
                    QUIET_WINDOW_MS / 1000,
                    entry.path().display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tests_command;

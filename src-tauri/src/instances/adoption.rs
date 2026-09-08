//! Adopting an already-installed DSH into a PHL instance (development spec
//! part 2). Where `create` builds an empty environment and `bundle` rebuilds
//! one from a manifest, adoption takes a *live* DSH_HOME the user already has
//! and wraps an instance around it — in one of two modes:
//!
//! - **Copy (recommended)** — the environment is duplicated into
//!   `<instance>/dsh-home`; the source DSH is left completely untouched, so
//!   a mistake costs nothing but disk.
//! - **External (advanced)** — PHL manages the DSH_HOME *in place*: it records
//!   the path, launches it with that as `DSH_HOME`, and reads plugins/config
//!   from it, but every destructive operation (snapshot restore, plugin
//!   writes, repair) is gated off. The user keeps owning that directory.
//!
//! `pack-installed` is a copy-mode variant that arrives with `.phlpack` (P1);
//! it is rejected here so the mode never lies about what the build can do.
//!
//! What adoption deliberately does NOT do: apply the global API binding. A
//! created instance "boots configured" by materializing the library; an
//! adopted one already carries the user's own `settings.yaml`, and silently
//! overwriting it would be exactly the destructive surprise adoption is
//! supposed to avoid. The adopted instance therefore reports as unbound until
//! the user opts in.
//!
//! Session movement: `All` travels inside the directory copy; `None` drops
//! the session store (and its derived caches) from it; `Selected` copies the
//! environment minus the session store, then re-adds exactly the chosen
//! conversation directories, located with the session engine's discovery walk
//! and copied **verbatim** (same ids). Adoption is a *migration* — the user's
//! own history arriving intact — not the cross-instance *distribution* the
//! re-identifying fork engine (`sessions::copy`) performs. That distinction
//! is why ids are preserved here and re-minted there (spec §3.3 vs §5.1).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use super::copy::skipped;
use super::{
    build_record, instance_dir, instances_root, sanitize_segment, AdoptedFrom, CloneProgress,
    InstanceManifest, InstanceRecord, InstanceSource, ManagementMode,
};
use crate::discovery::candidate::home_key;
use crate::discovery::{inspect, Confidence};
use crate::paths::PhlState;
use crate::versions::now_iso;

/* ------------------------------ wire types ------------------------------ */

/// The session policy chosen in the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStrategy {
    /// Copy the whole DSH_HOME including its session store.
    All,
    /// Copy the environment but exclude session persistence (and its
    /// derivable projection cache) — see `excludes_sessions`.
    None,
    /// Copy the environment minus the session store, then import exactly the
    /// conversation directories named in `AdoptionRequest::session_dirs`
    /// (spec §3.3). Verbatim directories — original ids are preserved, since
    /// adoption migrates the user's own history rather than distributing it.
    Selected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionRequest {
    /// The source DSH_HOME as chosen by the user (absolute path).
    pub source_home: String,
    /// ManagedCopy | External. PackInstalled and any other value are refused.
    pub mode: ManagementMode,
    pub session_strategy: SessionStrategy,
    /// Session directory names (`session-…`) to migrate when the strategy is
    /// `Selected`; ignored otherwise. `#[serde(default)]` so an older request
    /// shape still parses — it simply fails the "Selected needs a list" check.
    #[serde(default)]
    pub session_dirs: Vec<String>,
    /// Identity and environment shape (id, name, hue, port, profile, version,
    /// runtime, args, env). Adoption overwrites the management fields
    /// (`management_mode`, `source`, `external_home`, `adopted_from`); the
    /// rest is the user's naming, exactly as `create` receives it.
    pub manifest: InstanceManifest,
}

/// What the wizard shows before committing: the source, restated and sized
/// under the chosen policy, so the cost of a copy is a decision the user sees.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionPreview {
    pub source_home: String,
    pub mode: ManagementMode,
    pub session_strategy: SessionStrategy,
    pub profile: String,
    pub detected_version: Option<String>,
    pub plugin_count: usize,
    pub session_count: usize,
    /// Bytes the copy would move (source tree minus what the policy excludes).
    /// Zero for external mode, which moves nothing.
    pub copy_bytes: u64,
    /// Symlinked entries the copy skips (linked plugins that will not
    /// reproduce in the copy); surfaced so "adopted but a plugin is missing"
    /// is explainable rather than mysterious.
    pub symlink_entries: usize,
    pub warnings: Vec<String>,
    /// The honest note external mode must always show (spec §3.4).
    pub keeps_existing_sessions: bool,
    /// How many of the home's conversations would actually migrate under
    /// `Selected` (0 for the other strategies, which are expressed by
    /// `session_strategy` itself).
    pub selected_session_count: usize,
}

/// Adoption's result: the created record plus the provenance actually written.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionOutcome {
    pub record: InstanceRecord,
    pub adopted_from: AdoptedFrom,
}

/* ------------------------------- preview ------------------------------- */

/// The copy exclusion set for a session strategy (spike §8): with `None` and
/// `Selected`, the top-level session store is left out — `Selected` then
/// re-adds the chosen conversation directories after the walk (spec §3.3);
/// `All` travels together. The session projection cache under `storages/` is
/// handled separately by `SESSION_CACHE_ARTIFACTS` (it is nested, not
/// top-level), and drops together with the store for both excluding modes.
fn excludes_sessions(strategy: SessionStrategy) -> &'static [&'static str] {
    match strategy {
        SessionStrategy::None | SessionStrategy::Selected => &["sessions"],
        SessionStrategy::All => &[],
    }
}

/// Derived, session-specific storage artifacts skipped at ANY depth when the
/// session store is excluded (spike §2, §8). Dropping them keeps a "no
/// history" copy consistent — a cache pointing at absent sessions is worse
/// than none, which DSH rebuilds on first launch. Other `storages/` content
/// (e.g. `workspace.json`) is not session data and travels normally.
const SESSION_CACHE_ARTIFACTS: [&str; 2] = ["session_projcache", "session_projcache.json"];

/// The (top-level, deep) exclusion sets for a strategy. `None` and `Selected`
/// drop the session store at the root and its derived caches at any depth;
/// `Selected` then re-adds the chosen conversation trees through
/// `import_selected_sessions` (spec §3.3).
fn exclusion_sets(strategy: SessionStrategy) -> (&'static [&'static str], &'static [&'static str]) {
    match strategy {
        SessionStrategy::None | SessionStrategy::Selected => {
            (excludes_sessions(strategy), &SESSION_CACHE_ARTIFACTS)
        }
        SessionStrategy::All => (&[][..], &[][..]),
    }
}

/// The preview's shape-only source check: a real, addressable DSH home. The
/// managed-instance refusal is deliberately NOT here — a preview shows what a
/// copy would cost even for a home that turns out to be managed (the wizard
/// then explains it); only `adopt_instance` enforces the refusal.
fn check_home_shape(req: &AdoptionRequest) -> Result<PathBuf, String> {
    let raw = req.source_home.trim();
    if raw.is_empty() {
        return Err("未选择 DSH 目录".into());
    }
    let path = PathBuf::from(raw);
    if !path.is_dir() {
        return Err(format!("目录不存在: {}", path.display()));
    }
    if inspect::classify_home(&path) == Confidence::Invalid {
        return Err(format!("该目录不是一个 DSH 数据目录: {}", path.display()));
    }
    Ok(path)
}

#[tauri::command]
pub async fn preview_adoption(req: AdoptionRequest) -> Result<AdoptionPreview, String> {
    let source = check_home_shape(&req)?;
    if req.mode == ManagementMode::PackInstalled {
        return Err("整合包请通过「安装整合包」入口导入".into());
    }
    let external = req.mode == ManagementMode::External;
    if external && req.session_strategy != SessionStrategy::All {
        // Not dangerous, just meaningless — in-place keeps the home's own
        // sessions. The wizard hides the choice; a hand-built request must
        // still not compute a bogus exclusion.
        return Err("原地接入始终沿用现有 DSH_HOME，历史对话保持原样，无需选择迁移策略".into());
    }
    let strategy = if external {
        SessionStrategy::All
    } else {
        req.session_strategy
    };
    // A Selected preview is only meaningful with a concrete, valid selection;
    // normalize it before measuring so the wizard sees the real cost.
    let selected_dirs = if strategy == SessionStrategy::Selected {
        normalize_selected_dirs(&req.session_dirs)?
    } else {
        Vec::new()
    };
    let selected_dirs_for_walk = selected_dirs.clone();

    let measured = tokio::task::spawn_blocking(move || {
        let (root_ex, deep_ex) = exclusion_sets(strategy);
        let (mut copy_bytes, symlink_entries) = measure_copy(&source, root_ex, deep_ex);
        // Selected: the walk excludes the whole store; add back exactly the
        // chosen conversation trees (and prove they exist in the source).
        let selected_dirs = selected_dirs_for_walk;
        let mut missing = Vec::new();
        for dir in &selected_dirs {
            match locate_session_source(&source, dir) {
                Some(path) => copy_bytes += measure_tree(&path, &[]),
                None => missing.push(dir.clone()),
            }
        }
        let profile_choice = inspect::pick_profile(&source);
        let (profile, plugin_count, plugin_warnings, detected_version) = match &profile_choice {
            Some((name, dir)) => {
                let (count, warns) = inspect::declared_plugins(dir).unwrap_or((0, Vec::new()));
                (
                    name.clone(),
                    count,
                    warns,
                    inspect::detect_version(&source, Some(dir)),
                )
            }
            None => ("web".into(), 0, vec!["未找到 profile 目录".into()], None),
        };
        let session_count = inspect::count_sessions(&source);
        PreviewMeasure {
            copy_bytes,
            symlink_entries,
            profile,
            plugin_count,
            plugin_warnings,
            detected_version,
            session_count,
            missing_sessions: missing,
        }
    })
    .await
    .map_err(|e| format!("估算失败: {e}"))?;
    if !measured.missing_sessions.is_empty() {
        return Err(format!(
            "以下对话在源目录中不存在，可能已被删除：{}",
            measured.missing_sessions.join("、")
        ));
    }

    let mut warnings = measured.plugin_warnings;
    if measured.detected_version.is_none() {
        warnings.push("未能确定源 DSH 版本，接入后请核对运行环境".into());
    }
    if external {
        warnings.push("原地接入：PHL 直接使用该目录，历史对话保持可见".into());
    } else {
        let scope = match strategy {
            SessionStrategy::None => "（不含历史对话）".to_string(),
            SessionStrategy::Selected => {
                format!("（含选定的 {} 条历史对话）", selected_dirs.len())
            }
            SessionStrategy::All => String::new(),
        };
        warnings.push(format!(
            "将向 PHL 数据目录复制约 {}{scope}",
            human_bytes(measured.copy_bytes)
        ));
        if measured.symlink_entries > 0 {
            warnings.push(format!(
                "源目录含 {} 个符号链接（如本地 link 插件），复制时跳过，接入后可按需重装",
                measured.symlink_entries
            ));
        }
    }

    Ok(AdoptionPreview {
        source_home: req.source_home,
        mode: req.mode,
        session_strategy: strategy,
        profile: measured.profile,
        detected_version: measured.detected_version,
        plugin_count: measured.plugin_count,
        session_count: measured.session_count,
        copy_bytes: if external { 0 } else { measured.copy_bytes },
        symlink_entries: measured.symlink_entries,
        warnings,
        keeps_existing_sessions: external,
        selected_session_count: if strategy == SessionStrategy::Selected {
            selected_dirs.len()
        } else {
            0
        },
    })
}

/// Everything one blocking pass over the source can measure about a preview.
struct PreviewMeasure {
    copy_bytes: u64,
    symlink_entries: usize,
    profile: String,
    plugin_count: usize,
    plugin_warnings: Vec<String>,
    detected_version: Option<String>,
    session_count: usize,
    /// Selected dirs that no longer exist in the source home (a commit-time
    /// staleness signal for the preview).
    missing_sessions: Vec<String>,
}

/* -------------------------------- commit -------------------------------- */

#[tauri::command]
pub async fn adopt_instance(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    state: State<'_, PhlState>,
    req: AdoptionRequest,
    on_progress: Channel<CloneProgress>,
) -> Result<AdoptionOutcome, String> {
    let id = req.manifest.id.clone();
    let label = id.clone();
    let flag = Arc::new(AtomicBool::new(false));
    crate::resources::guarded(
        crate::resources::next_task_id("adopt"),
        "adopt",
        format!("接入本机 DSH → 实例 {label}"),
        vec![crate::resources::Resource::Instance(id.clone())],
        Some(flag.clone()),
        &locks,
        &tasks,
        move |_task| async move {
            let r = adopt_inner(&state.root(), req, &flag, &on_progress).await?;
            if let Ok(dir) = instance_dir(&state.root(), &id) {
                super::invalidate_disk_usage(&dir);
            }
            Ok(r)
        },
    )
    .await
}

async fn adopt_inner(
    root: &Path,
    req: AdoptionRequest,
    cancel: &Arc<AtomicBool>,
    on_progress: &Channel<CloneProgress>,
) -> Result<AdoptionOutcome, String> {
    let source = validate_source(root, &req).await?;
    if req.mode == ManagementMode::PackInstalled {
        return Err("整合包请通过「安装整合包」入口导入".into());
    }
    let external = req.mode == ManagementMode::External;
    if external && req.session_strategy != SessionStrategy::All {
        return Err("原地接入必须保留源 DSH_HOME 的全部数据".into());
    }
    // Selected: resolve + validate the list up front (sanitized names,
    // non-empty, deduplicated) so a bad request fails before staging.
    let selected_dirs = if req.session_strategy == SessionStrategy::Selected {
        normalize_selected_dirs(&req.session_dirs)?
    } else {
        Vec::new()
    };

    let mut manifest = req.manifest;
    let id = sanitize_segment(&manifest.id, "实例 id")?;
    manifest.id = id.clone();
    manifest.management_mode = req.mode;
    manifest.source = InstanceSource::Adopted;
    manifest.external_home = external.then(|| source.to_string_lossy().into_owned());
    let adopted_from = AdoptedFrom {
        dsh_home: source.to_string_lossy().into_owned(),
        detected_version: manifest_version_from_source(&source).await,
        adopted_at: now_iso(),
        mode: req.mode.as_str().to_string(),
    };
    manifest.adopted_from = Some(adopted_from.clone());

    // Identity fields PHL owns regardless of what the frontend sent.
    manifest.profile = {
        let trimmed = manifest.profile.trim();
        let chosen = if trimmed.is_empty() { "web" } else { trimmed };
        sanitize_segment(chosen, "profile 名")?
    };
    if manifest.created_at.trim().is_empty() {
        manifest.created_at = now_iso();
    }
    // An adopted instance boots with its own configuration, not the library.
    manifest.api = None;

    let dir = instance_dir(root, &id)?;
    if dir.exists() {
        return Err(format!("实例目录已存在: {id}"));
    }

    let staging = instances_root(root).join(format!(".phl-adopt-{id}"));
    let _ = std::fs::remove_dir_all(&staging);

    let commit = async {
        // The instance wrapper directories exist in both modes.
        for sub in ["workspace", "logs"] {
            std::fs::create_dir_all(staging.join(sub))
                .map_err(|e| format!("无法创建实例目录: {e}"))?;
        }
        if !external {
            if cancel.load(Ordering::SeqCst) {
                return Err("cancelled".into());
            }
            let (root_ex, deep_ex) = exclusion_sets(req.session_strategy);
            let exclude: Vec<String> = root_ex.iter().map(|s| s.to_string()).collect();
            let deep: Vec<String> = deep_ex.iter().map(|s| s.to_string()).collect();
            let dest = staging.join("dsh-home");
            let walk_source = source.clone();
            let walk_dest = dest.clone();
            let channel = on_progress.clone();
            let cancel = cancel.clone();
            let _ = tokio::task::spawn_blocking(move || {
                copy_home_sync(
                    &walk_source,
                    &walk_dest,
                    cancel.as_ref(),
                    &exclude,
                    &deep,
                    &channel,
                )
            })
            .await
            .map_err(|e| format!("复制线程异常退出: {e}"))??;
            // Selected: the walk dropped the whole store; re-add exactly the
            // chosen conversations, copied verbatim into the staged home
            // (same tree-discipline: symlinks skipped, nothing followed out).
            if !selected_dirs.is_empty() {
                let source = source.clone();
                let dest = dest.clone();
                let dirs = selected_dirs.clone();
                tokio::task::spawn_blocking(move || {
                    import_selected_sessions(&source, &dest, &dirs)
                })
                .await
                .map_err(|e| format!("会话导入线程异常退出: {e}"))??;
                // The bar's byte-total measured the walk only; the session
                // trees are small next to an environment — land it on 1.0.
                let _ = on_progress.send(CloneProgress {
                    progress: 1.0,
                    bytes_done: 0,
                    bytes_total: 0,
                });
            }
        }
        super::manifest::write_manifest(&staging, &manifest)
            .await
            .map_err(|e| format!("无法写入实例清单: {e}"))?;
        Ok::<(), String>(())
    }
    .await;

    if let Err(e) = commit {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    if cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err("cancelled".into());
    }
    tokio::fs::rename(&staging, &dir).await.map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("无法放置实例目录: {e}")
    })?;

    Ok(AdoptionOutcome {
        record: build_record(&dir, manifest).await,
        adopted_from,
    })
}

/* --------------------------- selected sessions -------------------------- */

/// Validate a `Selected` request's conversation list: each dir passes the same
/// filesystem whitelist the session engine uses, duplicates collapse, and an
/// empty selection is refused (it means "不迁移", which has its own strategy —
/// silently importing nothing would look like a lost selection).
fn normalize_selected_dirs(dirs: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::with_capacity(dirs.len());
    for d in dirs {
        let name = crate::sessions::sanitize_session_dir(d)?;
        if !out.contains(&name) {
            out.push(name);
        }
    }
    if out.is_empty() {
        return Err("未选择任何要迁移的历史对话".into());
    }
    Ok(out)
}

/// Find `<home>/sessions/<project>/<dir>` for one encoded session dir name.
/// Same discovery walk the engine's `locate_source` performs — kept local
/// (path-addressed) because an adoption source is a raw home, not an instance.
fn locate_session_source(home: &Path, dir: &str) -> Option<PathBuf> {
    let root = home.join("sessions");
    let Ok(projects) = std::fs::read_dir(&root) else {
        return None;
    };
    for project in projects.flatten() {
        let cand = project.path().join(dir);
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

/// Copy the chosen session trees from the source home into the staged
/// instance home verbatim (directories, project grouping, and ids intact).
/// Fails fast on the first missing source: adoption is one transaction, and
/// the staging rollback above turns a partial import into no instance.
fn import_selected_sessions(
    source: &Path,
    dest_home: &Path,
    dirs: &[String],
) -> Result<(), String> {
    for dir in dirs {
        let from = locate_session_source(source, dir)
            .ok_or_else(|| format!("历史对话 {dir} 在源目录中不存在，请重新选择"))?;
        let project_name = from
            .parent()
            .and_then(|p| p.file_name())
            .ok_or_else(|| "源会话项目目录名异常".to_string())?;
        let to = dest_home.join("sessions").join(project_name).join(dir);
        std::fs::create_dir_all(&to).map_err(|e| format!("创建会话目录失败 {}: {e}", dir))?;
        copy_session_tree(&from, &to)?;
    }
    Ok(())
}

/// Verbatim subtree copy for one session directory: files and dirs travel,
/// symlinks are skipped (same discipline as the home walk — never follow an
/// external target). Session stores are small; no progress stream here.
fn copy_session_tree(src: &Path, dst: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(src).map_err(|e| format!("读取会话目录失败: {e}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            copy_session_tree(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)
                .map_err(|e| format!("复制会话文件失败 {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// List the conversations a *source* DSH_HOME holds, for the wizard's
/// "选择对话" step. Path-addressed like the other manual inspection commands
/// (`inspect_dsh_home`), but read-only and shape-gated: the dir must be an
/// addressable DSH home before anything under it is walked.
#[tauri::command]
pub async fn list_adoption_sessions(
    path: String,
) -> Result<Vec<crate::sessions::SessionInfo>, String> {
    let raw = path.trim();
    if raw.is_empty() {
        return Err("未选择 DSH 目录".into());
    }
    let home = PathBuf::from(raw);
    if !home.is_dir() {
        return Err(format!("目录不存在: {}", home.display()));
    }
    if inspect::classify_home(&home) == Confidence::Invalid {
        return Err(format!("该目录不是一个 DSH 数据目录: {}", home.display()));
    }
    let listed = tokio::task::spawn_blocking(move || crate::sessions::list_in_home(&home).0)
        .await
        .map_err(|e| format!("列举会话失败: {e}"))?;
    Ok(listed)
}

/* ------------------------------ copy engine ----------------------------- */

/// Verify the source is (a) an addressable DSH home and (b) not already a
/// managed instance's home. Returns the path on success.
async fn validate_source(root: &Path, req: &AdoptionRequest) -> Result<PathBuf, String> {
    let raw = req.source_home.trim();
    if raw.is_empty() {
        return Err("未选择 DSH 目录".into());
    }
    let path = PathBuf::from(raw);
    if !path.is_dir() {
        return Err(format!("目录不存在: {}", path.display()));
    }
    match inspect::classify_home(&path) {
        Confidence::Invalid => {
            return Err(format!("该目录不是一个 DSH 数据目录: {}", path.display()))
        }
        // Low/Medium were already surfaced as warnings by discovery; the user
        // confirmed them in the wizard, so adoption honours the choice.
        Confidence::Low | Confidence::Medium | Confidence::High => {}
    }
    // Refuse a home that is already a managed instance's `dsh-home`: adopting
    // it would nest PHL inside PHL and hand two owners to one tree. The scan
    // told the UI (alreadyManaged); this is the backend gate the scan cannot
    // be trusted to enforce, re-checked against the live root.
    let key = home_key(&path);
    if let Some(owner) = crate::discovery::managed_instance_for(root, &key).await {
        return Err(format!(
            "该 DSH 环境已由 PHL 实例「{}」管理，无需再次接入",
            owner.name
        ));
    }
    Ok(path)
}

/// Synchronous home copy: excluded top-level names are dropped, symlinks are
/// counted and skipped (never followed into an external target), everything
/// else travels. Progress goes straight onto the IPC channel, which is `Send`.
fn copy_home_sync(
    source: &Path,
    dest: &Path,
    flag: &AtomicBool,
    exclude: &[String],
    deep: &[String],
    on_progress: &Channel<CloneProgress>,
) -> Result<(u64, u64, usize), String> {
    let excludes: Vec<&str> = exclude.iter().map(|s| s.as_str()).collect();
    let deep_refs: Vec<&str> = deep.iter().map(|s| s.as_str()).collect();
    let total = measure_copy(source, &excludes, &deep_refs).0;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut done = 0u64;
    let mut symlinks = 0usize;
    let report = |done: u64, total: u64| {
        let _ = on_progress.send(CloneProgress {
            progress: if total == 0 {
                1.0
            } else {
                (done as f64 / total as f64).min(1.0)
            },
            bytes_done: done,
            bytes_total: total,
        });
    };
    copy_walk(
        source,
        dest,
        flag,
        exclude,
        deep,
        true,
        &mut done,
        total,
        &report,
        &mut symlinks,
    )?;
    Ok((done, total, symlinks))
}

#[allow(clippy::too_many_arguments)]
fn copy_walk(
    src: &Path,
    dst: &Path,
    flag: &AtomicBool,
    exclude: &[String],
    deep: &[String],
    at_root: bool,
    done: &mut u64,
    total: u64,
    on_progress: &dyn Fn(u64, u64),
    symlinks: &mut usize,
) -> Result<(), String> {
    let entries = std::fs::read_dir(src).map_err(|e| format!("读取源目录失败: {e}"))?;
    for entry in entries.flatten() {
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Top-level strategy drops + PHL run-state at this tree's root only;
        // the derived session-cache artifacts are dropped at any depth.
        let root_skip = at_root && (exclude.contains(&name) || skipped(&name));
        if root_skip || deep.contains(&name) {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        let ft = entry.file_type().map_err(|e| e.to_string())?;
        if ft.is_symlink() {
            *symlinks += 1;
            continue;
        }
        if ft.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            copy_walk(
                &from,
                &to,
                flag,
                exclude,
                deep,
                false,
                done,
                total,
                on_progress,
                symlinks,
            )?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to).map_err(|e| format!("复制失败 {}: {e}", from.display()))?;
            if let Ok(meta) = entry.metadata() {
                *done += meta.len();
            }
            on_progress(*done, total);
        }
    }
    Ok(())
}

/// Bytes the copy would move under both exclusion sets, plus the symlink count.
fn measure_copy(source: &Path, exclude: &[&str], deep: &[&str]) -> (u64, usize) {
    let mut total = 0u64;
    let mut symlinks = 0usize;
    let Ok(entries) = std::fs::read_dir(source) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if exclude.contains(&name.as_str()) || skipped(&name) || deep.contains(&name.as_str()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false) {
            symlinks += 1;
        } else if meta.is_dir() {
            total += measure_tree(&entry.path(), deep);
        } else {
            total += meta.len();
        }
    }
    (total, symlinks)
}

fn measure_tree(dir: &Path, deep: &[&str]) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if deep.contains(&name.as_str()) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_symlink() {
            continue;
        }
        if meta.is_dir() {
            total += measure_tree(&entry.path(), deep);
        } else {
            total += meta.len();
        }
    }
    total
}

async fn manifest_version_from_source(source: &Path) -> Option<String> {
    let dir = source.to_path_buf();
    tokio::task::spawn_blocking(move || {
        inspect::pick_profile(&dir).and_then(|(_, pd)| inspect::detect_version(&dir, Some(&pd)))
    })
    .await
    .ok()
    .flatten()
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::strip_verbatim;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-adopt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A source DSH_HOME with one plugin, one session, and a projection cache
    /// — enough to prove what the copy takes and what it leaves behind.
    fn fake_dsh_home(dir: &Path) {
        let profile = dir.join("profiles").join("web");
        std::fs::create_dir_all(profile.join("node_modules").join("dsh-cool")).unwrap();
        std::fs::write(
            profile.join("package.json"),
            r#"{"dsh":{"profile":{"bundles":["@deepseek-ai/dsh-base","dsh-cool"]}}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("settings.yaml"), "ui: onboarding\n").unwrap();
        // Two session artifacts under a project directory, each with a
        // distinct marker file so a selective import is observable.
        let project = dir.join("sessions").join("--P--");
        let sess_a = project.join("session-a");
        std::fs::create_dir_all(&sess_a).unwrap();
        std::fs::write(sess_a.join("session.jsonl.zstd"), b"frame-a").unwrap();
        let sess_b = project.join("session-b");
        std::fs::create_dir_all(&sess_b).unwrap();
        std::fs::write(sess_b.join("session.jsonl"), b"frame-b").unwrap();
        // Session-derived cache (dropped with strategy None) plus unrelated
        // storage that must survive the exclusion.
        std::fs::create_dir_all(dir.join("storages").join("session_projcache")).unwrap();
        std::fs::write(dir.join("storages").join("workspace.json"), b"{}").unwrap();
    }

    fn req(
        dir: &Path,
        mode: ManagementMode,
        strategy: SessionStrategy,
        id: &str,
    ) -> AdoptionRequest {
        AdoptionRequest {
            source_home: dir.to_string_lossy().into_owned(),
            mode,
            session_strategy: strategy,
            session_dirs: Vec::new(),
            manifest: InstanceManifest {
                schema_version: 0,
                id: id.into(),
                name: "接入的实例".into(),
                note: None,
                kind: "sandbox".into(),
                hue: 210,
                version_id: "0.1.2-rc.1".into(),
                runtime_id: "node-22".into(),
                port: 7100,
                auto_port: false,
                profile: "web".into(),
                created_at: String::new(),
                last_run_at: None,
                total_runtime: 0,
                favorite: false,
                env: Default::default(),
                args: Vec::new(),
                api: None,
                management_mode: mode,
                source: InstanceSource::Adopted,
                external_home: None,
                adopted_from: None,
            },
        }
    }

    fn channel() -> Channel<CloneProgress> {
        Channel::new(|_| Ok(()))
    }

    #[tokio::test]
    async fn copy_mode_duplicates_the_home_and_leaves_the_source_intact() {
        let root = temp("copy");
        let source = root.join("ext-dsh");
        std::fs::create_dir_all(&source).unwrap();
        fake_dsh_home(&source);
        std::fs::create_dir_all(root.join("instances")).unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let outcome = adopt_inner(
            &root,
            req(
                &source,
                ManagementMode::ManagedCopy,
                SessionStrategy::All,
                "ad-1",
            ),
            &flag,
            &channel(),
        )
        .await
        .unwrap();

        let inst = root.join("instances").join("ad-1");
        assert!(inst.join("dsh-home").join("settings.yaml").exists());
        assert!(
            inst.join("dsh-home").join("sessions").exists(),
            "All strategy copies the session store"
        );
        assert_eq!(
            outcome.record.dsh_home,
            inst.join("dsh-home").to_string_lossy()
        );
        assert_eq!(
            outcome.record.manifest.management_mode,
            ManagementMode::ManagedCopy
        );
        assert_eq!(outcome.adopted_from.mode, "managed-copy");
        // The source is untouched (spec test: Copy 后原 DSH 未改变).
        assert!(source.join("settings.yaml").exists());
        assert!(source.join("sessions").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn none_strategy_excludes_sessions_and_projection_cache() {
        let root = temp("none");
        let source = root.join("ext-dsh");
        std::fs::create_dir_all(&source).unwrap();
        fake_dsh_home(&source);
        std::fs::create_dir_all(root.join("instances")).unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        adopt_inner(
            &root,
            req(
                &source,
                ManagementMode::ManagedCopy,
                SessionStrategy::None,
                "ad-2",
            ),
            &flag,
            &channel(),
        )
        .await
        .unwrap();

        let home = root.join("instances").join("ad-2").join("dsh-home");
        assert!(home.join("settings.yaml").exists());
        assert!(home.join("profiles").is_dir(), "the environment travels");
        assert!(!home.join("sessions").exists(), "session store excluded");
        assert!(
            !home.join("storages").join("session_projcache").exists(),
            "derived session cache excluded"
        );
        assert!(
            home.join("storages").join("workspace.json").exists(),
            "unrelated storage is not collateral damage"
        );
        // And the source still has them (we never delete from the user's home).
        assert!(source.join("sessions").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn external_mode_records_the_home_and_creates_no_copy() {
        let root = temp("ext");
        let source = root.join("ext-dsh");
        std::fs::create_dir_all(&source).unwrap();
        fake_dsh_home(&source);
        std::fs::create_dir_all(root.join("instances")).unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let outcome = adopt_inner(
            &root,
            req(
                &source,
                ManagementMode::External,
                SessionStrategy::All,
                "ad-3",
            ),
            &flag,
            &channel(),
        )
        .await
        .unwrap();

        let inst = root.join("instances").join("ad-3");
        assert!(
            !inst.join("dsh-home").exists(),
            "external creates no copy tree"
        );
        // Verify by resolved directory *identity*, not raw string equality. The
        // record stores the source path the user pointed at verbatim; on Windows
        // that can arrive in the 8.3 short form (e.g. `...\RUNNER~1\...` from a
        // temp dir) while `canonicalize` returns the long form
        // (`...\runneradmin\...`), so comparing the strings directly was a
        // representation clash, not a real defect. Canonicalising both sides to
        // their unique final path still proves the external home resolves to the
        // user's actual source directory (and fails loudly if it points anywhere
        // that does not exist).
        let want = strip_verbatim(&std::fs::canonicalize(&source).unwrap());
        let got = strip_verbatim(
            &std::fs::canonicalize(std::path::Path::new(&outcome.record.dsh_home)).unwrap(),
        );
        assert_eq!(got, want, "the record points at the user's own home");
        assert_eq!(
            outcome.record.manifest.management_mode,
            ManagementMode::External
        );
        assert!(outcome.record.manifest.external_home.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn adopting_an_already_managed_home_is_refused() {
        let root = temp("managed");
        std::fs::create_dir_all(root.join("instances")).unwrap();
        // A managed instance whose dsh-home is the thing we then try to adopt.
        let inst = root.join("instances").join("mine");
        let home = inst.join("dsh-home");
        std::fs::create_dir_all(&home).unwrap();
        fake_dsh_home(&home);
        crate::instances::manifest::write_manifest(&inst, &{
            let mut m = req(
                &home,
                ManagementMode::ManagedCopy,
                SessionStrategy::All,
                "mine",
            )
            .manifest;
            m.id = "mine".into();
            m
        })
        .await
        .unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let err = adopt_inner(
            &root,
            req(
                &home,
                ManagementMode::ManagedCopy,
                SessionStrategy::All,
                "ad-dup",
            ),
            &flag,
            &channel(),
        )
        .await
        .unwrap_err();
        assert!(err.contains("已由 PHL 实例"), "got: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn pack_install_mode_is_refused() {
        let root = temp("refuse");
        let source = root.join("ext-dsh");
        std::fs::create_dir_all(&source).unwrap();
        fake_dsh_home(&source);
        let flag = Arc::new(AtomicBool::new(false));

        let pack = adopt_inner(
            &root,
            req(
                &source,
                ManagementMode::PackInstalled,
                SessionStrategy::All,
                "ad-p",
            ),
            &flag,
            &channel(),
        )
        .await
        .unwrap_err();
        assert!(pack.contains("整合包"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn selected_strategy_migrates_only_the_chosen_conversations() {
        let root = temp("selected");
        let source = root.join("ext-dsh");
        std::fs::create_dir_all(&source).unwrap();
        fake_dsh_home(&source);
        std::fs::create_dir_all(root.join("instances")).unwrap();

        let mut r = req(
            &source,
            ManagementMode::ManagedCopy,
            SessionStrategy::Selected,
            "ad-s",
        );
        r.session_dirs = vec!["session-a".into()];
        let flag = Arc::new(AtomicBool::new(false));
        adopt_inner(&root, r, &flag, &channel()).await.unwrap();

        let home = root.join("instances").join("ad-s").join("dsh-home");
        assert!(
            home.join("settings.yaml").exists(),
            "the environment travels"
        );
        let kept = home.join("sessions").join("--P--").join("session-a");
        assert!(
            kept.join("session.jsonl.zstd").exists(),
            "the chosen conversation lands verbatim, same id"
        );
        assert!(
            !home
                .join("sessions")
                .join("--P--")
                .join("session-b")
                .exists(),
            "the unchosen one does not"
        );
        assert!(
            !home.join("storages").join("session_projcache").exists(),
            "the derived session cache still drops with the store"
        );
        assert!(home.join("storages").join("workspace.json").exists());
        // The source keeps both (adoption never deletes from the user's home).
        assert!(source
            .join("sessions")
            .join("--P--")
            .join("session-b")
            .exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn selected_strategy_rejects_bad_selections() {
        let root = temp("selected-bad");
        let source = root.join("ext-dsh");
        std::fs::create_dir_all(&source).unwrap();
        fake_dsh_home(&source);
        std::fs::create_dir_all(root.join("instances")).unwrap();
        let flag = Arc::new(AtomicBool::new(false));

        // Empty selection → refuse, not "silently import nothing".
        let empty = req(
            &source,
            ManagementMode::ManagedCopy,
            SessionStrategy::Selected,
            "ad-e",
        );
        let err = adopt_inner(&root, empty, &flag, &channel())
            .await
            .unwrap_err();
        assert!(err.contains("未选择"), "got: {err}");

        // A dir that is not in the source home → refuse with staging rolled
        // back (no instance directory left behind).
        let mut ghost = req(
            &source,
            ManagementMode::ManagedCopy,
            SessionStrategy::Selected,
            "ad-g",
        );
        ghost.session_dirs = vec!["session-ghost".into()];
        let err = adopt_inner(&root, ghost, &flag, &channel())
            .await
            .unwrap_err();
        assert!(err.contains("不存在"), "got: {err}");
        assert!(
            !root.join("instances").join("ad-g").exists(),
            "failed adoption leaves no half instance"
        );

        // A traversal-shaped name never reaches the filesystem layer.
        let mut evil = req(
            &source,
            ManagementMode::ManagedCopy,
            SessionStrategy::Selected,
            "ad-x",
        );
        evil.session_dirs = vec!["..".into()];
        let err = adopt_inner(&root, evil, &flag, &channel())
            .await
            .unwrap_err();
        assert!(err.contains("会话目录名"), "got: {err}");

        // External + Selected is meaningless (the home stays the user's own).
        let mut ext = req(
            &source,
            ManagementMode::External,
            SessionStrategy::Selected,
            "ad-ext",
        );
        ext.session_dirs = vec!["session-a".into()];
        let err = adopt_inner(&root, ext, &flag, &channel())
            .await
            .unwrap_err();
        assert!(err.contains("全部数据"), "got: {err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn measure_copy_excludes_named_roots_and_counts_symlinks() {
        let dir = temp("measure");
        std::fs::create_dir_all(dir.join("keep")).unwrap();
        std::fs::write(dir.join("keep").join("f"), vec![0u8; 100]).unwrap();
        std::fs::create_dir_all(dir.join("sessions").join("x")).unwrap();
        std::fs::write(dir.join("sessions").join("big"), vec![0u8; 500]).unwrap();
        let (bytes, syms) = measure_copy(&dir, &["sessions"], &[]);
        assert_eq!(bytes, 100, "only the kept tree is measured");
        assert_eq!(syms, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_v2_round_trips_and_v1_defaults_are_backward() {
        // A v1 manifest (no new fields) parses into the v2 defaults that mean
        // "the old world": a PHL-owned copy that was created, never adopted.
        let v1 = r#"{"schemaVersion":1,"id":"a","name":"A","kind":"sandbox","hue":0,
            "versionId":"v","runtimeId":"r","port":1,"autoPort":false,"profile":"web",
            "createdAt":"now","env":{},"args":[]}"#;
        let m: InstanceManifest = serde_json::from_str(v1).unwrap();
        assert_eq!(m.management_mode, ManagementMode::ManagedCopy);
        assert_eq!(m.source, InstanceSource::Created);
        assert!(m.external_home.is_none());
        // And the mode serializes in the spec's kebab vocabulary.
        assert_eq!(
            serde_json::to_string(&ManagementMode::PackInstalled).unwrap(),
            "\"pack-installed\""
        );
    }
}

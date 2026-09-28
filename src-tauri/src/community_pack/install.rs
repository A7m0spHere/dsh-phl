//! Installation: extraction under the authoritative-machine-file rules,
//! the always-rebuilt profile package.json (bundle stack included), and the
//! retriable dependency phase.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::State;

use super::{
    forbidden_entry, unsafe_entry, CommunityManifest, CommunityPackPreview, ImportMarker,
    MAX_SINGLE_ENTRY, MAX_UNCOMPRESSED_TOTAL,
};
use crate::paths::{sanitize_segment, PhlState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityInstallRequest {
    /// Instance identity the user owns (id, hue, port, autoPort). The
    /// environment comes from the pack.
    pub instance: crate::instances::InstanceManifest,
    /// Path to the same pack the preview read; re-verified against the
    /// plan hash before anything is written.
    pub path: String,
    /// The preview this request executes. Changing the pack after preview
    /// changes its sha256 and the install refuses (D12).
    pub pack_sha256: String,
    /// Target DSH version: the pack's pin, or the user's explicit choice
    /// for packs that name none. Empty → refuse.
    #[serde(default)]
    pub dsh_version: String,
    /// Install dependencies in the same run (phase B). Unchecked leaves the
    /// instance needsDependencies and `prepare` finishes it later.
    #[serde(default = "default_true")]
    pub install_dependencies: bool,
    /// Ignore missing/blocking diagnostics after they were shown? Never:
    /// community packs install only through a clean plan.
    #[serde(default)]
    pub allow_missing: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommunityInstallOutcome {
    pub record: crate::instances::InstanceRecord,
    /// `readyToLaunch` when phase B ran clean; `needsDependencies` when the
    /// instance was created but dependency materialisation is pending or
    /// failed. Registration is the commit: a phase-B failure keeps it.
    pub readiness: String,
    pub dependencies_installed: usize,
    pub dependency_failures: Vec<String>,
    pub sessions_imported: usize,
}

/// Extract one archive entry to `dest`, streaming with hard caps on the
/// decompressed bytes. Mirrors pack-core's `confine` +
/// `stream_entry_to_file` invariants.
fn extract_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    dest: &Path,
    running_total: &mut u64,
) -> Result<(), String> {
    let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
    if entry.is_dir() {
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::File::create(dest).map_err(|e| format!("无法写入 {dest:?}: {e}"))?;
    let mut buf = [0u8; 65536];
    let mut written = 0u64;
    loop {
        let n = entry
            .read(&mut buf)
            .map_err(|e| format!("解压读取失败: {e}"))?;
        if n == 0 {
            break;
        }
        written += n as u64;
        *running_total += n as u64;
        if written > MAX_SINGLE_ENTRY || *running_total > MAX_UNCOMPRESSED_TOTAL {
            let _ = std::fs::remove_file(dest);
            return Err("解压内容超过限额：拒绝导入".into());
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("解压写入失败: {e}"))?;
    }
    Ok(())
}

/// The npm/git dependency object the profile package.json gets for one
/// manifest entry — the single authority for both the rebuilt machine file
/// and the phase-B verification. npm's actual forms: npm deps carry the
/// bare pin; git deps carry `github:owner/repo#<sha>` — the package name
/// is the dependency KEY, never a colon prefix inside the value (R2-01:
/// the old `name:github:…` value parsed as a local directory).
fn planned_dependency_value(coordinate: &str, pin: &str) -> serde_json::Value {
    if coordinate.starts_with("github:") {
        serde_json::Value::String(format!("{coordinate}#{pin}"))
    } else {
        serde_json::Value::String(pin.to_string())
    }
}

/// Builds the profile's `package.json` from the manifest — always, even
/// with zero dependencies — so the machine file on disk can never carry a
/// dependency the plan did not approve (CR-02), and so DSH's loader finds
/// the ordered bundle stack under `dsh.profile.bundles` (CR-01).
fn build_profile_package_json(
    manifest: &CommunityManifest,
    dependencies: &[super::PackDependency],
) -> serde_json::Value {
    let deps = planned_dependencies_map(dependencies);
    serde_json::json!({
        "name": format!("dsh-profile-{}", manifest.name),
        "private": true,
        "dependencies": deps,
        // The ordered layer stack DSH mounts: verbatim from the manifest.
        "dsh": { "profile": { "bundles": manifest.bundles } }
    })
}

#[tauri::command]
pub async fn preview_community_pack(
    _phl: State<'_, PhlState>,
    path: String,
) -> Result<CommunityPackPreview, String> {
    let path = PathBuf::from(path);
    tokio::task::spawn_blocking(move || super::plan::build_preview(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn install_inner(
    root: &Path,
    task: &crate::resources::Task,
    flag: &Arc<AtomicBool>,
    req: CommunityInstallRequest,
    on_progress: &(dyn Fn(f64) + Send + Sync),
) -> Result<CommunityInstallOutcome, String> {
    let pack_path = PathBuf::from(&req.path);
    // Re-validate against the previewed bytes first: anything else would be
    // executing a plan for a pack the user never saw.
    let preview: CommunityPackPreview = tokio::task::spawn_blocking({
        let pack_path = pack_path.clone();
        move || super::plan::build_preview(&pack_path)
    })
    .await
    .map_err(|e| e.to_string())??;
    if preview.pack_sha256 != req.pack_sha256 {
        return Err("包内容与预览时不一致（哈希不同）：请重新预览后再安装".into());
    }
    if !preview.blocked.is_empty() && !req.allow_missing {
        return Err(format!(
            "该包存在无法支持的项，拒绝导入：{}",
            preview.blocked.join("；")
        ));
    }
    if req.dsh_version.trim().is_empty() {
        return Err("未指定目标 DSH 版本（包未固定版本时必须显式选择）".into());
    }
    let target_version = crate::versions::install::sanitize_version(&req.dsh_version)?;
    if !tokio::fs::try_exists(
        root.join("versions")
            .join(&target_version)
            .join("phl-install.json"),
    )
    .await
    .unwrap_or(false)
    {
        return Err(format!("DSH {target_version} 未安装，请先在版本页安装"));
    }

    let id = sanitize_segment(&req.instance.id, "实例 id")?;
    let dest = crate::instances::instance_dir(root, &id)?;
    if dest.exists() {
        return Err(format!("实例目录已存在: {id}"));
    }
    let staging = crate::instances::instances_root(root).join(format!(".phl-packc-{id}"));
    let _ = tokio::fs::remove_dir_all(&staging).await;

    task.set_phase("extracting");
    let extract_report = tokio::task::spawn_blocking({
        let pack_path = pack_path.clone();
        let staging = staging.clone();
        let preview = preview.clone();
        let flag = Arc::clone(flag);
        move || -> Result<(usize, usize), String> {
            let (mut archive, manifest, _cv, _hash, _sz) = super::plan::open_pack(&pack_path)?;
            let home = staging.join("dsh-home");
            let profile = home.join("profiles").join("web");
            let mut running = 0u64;
            let mut overrides = 0usize;
            let mut home_n = 0usize;
            let mut claimed_targets: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for i in 0..archive.len() {
                if flag.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err("cancelled".into());
                }
                let name = {
                    let entry = archive.by_index(i).map_err(|e| e.to_string())?;
                    entry.name().to_string()
                };
                let namespace = if name.starts_with("overrides/") {
                    "overrides"
                } else if name.starts_with("home/") {
                    "home"
                } else {
                    continue; // machine snapshots & unknown roots: unmapped
                };
                let rel = name
                    .strip_prefix("overrides/")
                    .or_else(|| name.strip_prefix("home/"))
                    .unwrap_or("");
                if unsafe_entry(rel) {
                    return Err(format!("不安全的归档路径 {name}"));
                }
                if forbidden_entry(rel).is_some() {
                    return Err(format!("包内包含禁止落盘的内容（{name}）：拒绝导入"));
                }
                // CR-02: the profile's own machine files are the manifest's,
                // no matter what the archive carries.
                if super::is_authoritative_machine_path(namespace, rel) {
                    continue;
                }
                let Some(target_rel) = crate::community_pack::target_rel_path(namespace, rel)
                else {
                    continue;
                };
                // CR-03: live re-check — the preview already blocked known
                // collisions, but the plan is only as good as its re-check.
                if !claimed_targets.insert(target_rel.clone()) {
                    return Err(format!(
                        "多个归档条目映射到同一目标文件 {target_rel}：拒绝导入"
                    ));
                }
                let dest = home.join(&target_rel);
                extract_entry(&mut archive, i, &dest, &mut running)?;
                if namespace == "overrides" {
                    overrides += 1;
                } else {
                    home_n += 1;
                }
            }

            // The authoritative machine files, rebuilt unconditionally:
            std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
            let patch_exists = profile.join("cordis.patch.yml").exists();
            if !patch_exists {
                if let Some(patch) = &manifest.patch {
                    std::fs::write(profile.join("cordis.patch.yml"), patch)
                        .map_err(|e| format!("无法写入 cordis.patch.yml: {e}"))?;
                }
            }
            // Always write package.json — the bundle stack rides in it, and
            // an empty dependency set is a plan too (CR-01/CR-02).
            let package_json = build_profile_package_json(&manifest, &preview.dependencies);
            std::fs::write(
                profile.join("package.json"),
                serde_json::to_vec_pretty(&package_json).map_err(|e| e.to_string())?,
            )
            .map_err(|e| format!("无法写入 package.json: {e}"))?;
            Ok((overrides, home_n))
        }
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = extract_report;

    // The copy's manifest: pack provenance, PHL bindings.
    let mut manifest = req.instance.clone();
    manifest.id = id.clone();
    manifest.version_id =
        crate::versions::install::bound_id_for_version(&target_version).unwrap_or_default();
    // Runtime: the instance request carries it (user picked a precise one).
    manifest.profile = "web".into();
    manifest.management_mode = crate::instances::ManagementMode::PackInstalled;
    manifest.source = crate::instances::InstanceSource::Phlpack;
    manifest.adopted_from = Some(crate::instances::AdoptedFrom {
        dsh_home: format!("dspack:{}@{}", preview.name, preview.version),
        detected_version: Some(target_version.clone()),
        adopted_at: crate::versions::now_iso(),
        mode: "community-import".into(),
    });

    // Import record: what the plan was and how far the install got. The
    // planned dependency set rides with it so phase B can verify the
    // on-disk package.json before running npm (CR-02, D15).
    let planned_dependencies = planned_dependencies_map(&preview.dependencies);
    let marker = ImportMarker {
        format: "dspack".into(),
        pack_sha256: preview.pack_sha256.clone(),
        container_version: preview.container_version,
        manifest_version: preview.manifest_version,
        bundles: preview.bundles.clone(),
        planned_dependencies,
        dependencies_failed: Vec::new(),
        // Phase B only promotes the stage after it actually finishes.
        stage: "needsDependencies".into(),
    };
    std::fs::write(
        staging.join("phl-import.json"),
        serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if let Err(e) = crate::instances::manifest::write_manifest(&staging, &manifest).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }
    tokio::fs::rename(&staging, &dest)
        .await
        .map_err(|e| format!("无法放置实例目录: {e}"))?;

    // COMMIT POINT — the instance exists from here on. Phase-B failures do
    // not delete it; they leave it needsDependencies and retriable in place.
    crate::instances::invalidate_disk_usage(&dest);

    let mut outcome = CommunityInstallOutcome {
        record: crate::instances::build_record(&dest, manifest).await,
        readiness: "needsDependencies".into(),
        dependencies_installed: 0,
        dependency_failures: Vec::new(),
        sessions_imported: 0,
    };

    if req.install_dependencies {
        match prepare_dependencies(root, task, flag, &dest, on_progress).await {
            Ok(count) => {
                outcome.dependencies_installed = count;
                outcome.readiness = "readyToLaunch".into();
            }
            Err(failures) => {
                outcome.dependency_failures = failures;
            }
        }
    }
    Ok(outcome)
}

/// The dependency map exactly as the rebuilt package.json carries it — the
/// single definition both the writer and the phase-B verifier use.
fn planned_dependencies_map(
    dependencies: &[super::PackDependency],
) -> serde_json::Map<String, serde_json::Value> {
    let mut deps = serde_json::Map::new();
    for d in dependencies {
        if d.package_name.is_empty() {
            continue;
        }
        deps.insert(
            d.package_name.clone(),
            planned_dependency_value(&d.coordinate, &d.pin),
        );
    }
    deps
}

/// Phase B: materialise the profile's pinned dependencies with the version
/// pipeline's npm (fixed argv, --ignore-scripts). Runs against an *existing*
/// instance directory and is retriable in place; it never creates anything.
///
/// Before npm runs, the on-disk package.json is verified against the plan
/// recorded in `phl-import.json` — a file that drifted from the approved
/// dependency set fails here instead of executing an unplanned install
/// (CR-02/D15).
async fn prepare_dependencies(
    root: &Path,
    task: &crate::resources::Task,
    flag: &Arc<AtomicBool>,
    instance_dir: &Path,
    on_progress: &(dyn Fn(f64) + Send + Sync),
) -> Result<usize, Vec<String>> {
    prepare_dependencies_with_resolver(
        root,
        task,
        flag,
        instance_dir,
        on_progress,
        &GithubShortShaResolver,
    )
    .await
}

/// The git short-SHA resolver, as a trait so tests can stub it (R2-01).
pub(crate) trait ShortShaResolver: Send + Sync {
    fn resolve<'a>(
        &'a self,
        repo: &'a str,
        pin: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>>;
}

struct GithubShortShaResolver;

impl ShortShaResolver for GithubShortShaResolver {
    fn resolve<'a>(
        &'a self,
        repo: &'a str,
        pin: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + 'a>> {
        Box::pin(async move { resolve_short_commit_via_api(repo, pin).await })
    }
}

/// Phase B with an injectable git short-SHA resolver: production uses the
/// GitHub API, tests use controlled stubs (R2-01).
async fn prepare_dependencies_with_resolver(
    root: &Path,
    task: &crate::resources::Task,
    flag: &Arc<AtomicBool>,
    instance_dir: &Path,
    on_progress: &(dyn Fn(f64) + Send + Sync),
    resolver: &dyn ShortShaResolver,
) -> Result<usize, Vec<String>> {
    prepare_dependencies_full(root, task, flag, instance_dir, on_progress, resolver, None).await
}

/// Variant used by the retry command with a PRE-RESOLVED npm node — the
/// runtime that node belongs to is what the command locked (R2-06).
async fn prepare_dependencies_with_node(
    root: &Path,
    task: &crate::resources::Task,
    flag: &Arc<AtomicBool>,
    instance_dir: &Path,
    on_progress: &(dyn Fn(f64) + Send + Sync),
    node: Option<&Path>,
) -> Result<usize, Vec<String>> {
    prepare_dependencies_full(
        root,
        task,
        flag,
        instance_dir,
        on_progress,
        &GithubShortShaResolver,
        node,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_dependencies_full(
    root: &Path,
    task: &crate::resources::Task,
    flag: &Arc<AtomicBool>,
    instance_dir: &Path,
    on_progress: &(dyn Fn(f64) + Send + Sync),
    resolver: &dyn ShortShaResolver,
    pre_resolved_node: Option<&Path>,
) -> Result<usize, Vec<String>> {
    let profile = instance_dir.join("dsh-home").join("profiles").join("web");
    let package_json = profile.join("package.json");

    // The import marker is the authority for what phase B may do. A
    // missing or CORRUPT marker is an explicit failure — never an empty
    // plan that promotes the instance to ready (R2-02).
    let marker_path = instance_dir.join("phl-import.json");
    let marker: ImportMarker = match tokio::fs::read_to_string(&marker_path).await {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|e| vec![format!("导入记录损坏，无法确认计划依赖集: {e}")])?,
        Err(e) => return Err(vec![format!("导入记录不可读取: {e}")]),
    };
    let planned = marker.planned_dependencies.clone();

    let body = match tokio::fs::read_to_string(&package_json).await {
        Ok(body) => body,
        // Missing machine file: rebuild it from the marker's authoritative
        // plan when one exists; only a genuinely dependency-free plan may
        // proceed without the file (R2-02).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if planned.is_empty() {
                // A dependency-free plan whose machine file vanished: the
                // bundle stack still matters (CR-01), so rebuild it.
                let rebuilt = serde_json::json!({
                    "name": "dsh-profile-community-pack",
                    "private": true,
                    "dependencies": planned,
                    "dsh": { "profile": { "bundles": marker.bundles } }
                });
                write_json_atomic(
                    &package_json,
                    &serde_json::to_vec_pretty(&rebuilt).map_err(|e| vec![e.to_string()])?,
                )?;
                return update_stage(instance_dir, Vec::new(), true)
                    .map(|_| 0)
                    .map_err(|m| vec![m]);
            }
            return Err(vec![format!(
                "profile package.json 缺失且计划含 {} 项依赖：无法安全继续，请重新导入整合包",
                planned.len()
            )]);
        }
        // Unreadable is NOT "no dependencies" — that read must not become a
        // silent green light (CR-06).
        Err(e) => return Err(vec![format!("profile package.json 不可读取: {e}")]),
    };
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| vec![format!("profile package.json 解析失败: {e}")])?;
    let on_disk = parsed
        .get("dependencies")
        .and_then(|d| d.as_object())
        .cloned()
        .unwrap_or_default();

    // Verify against the plan recorded at install time.
    if on_disk != planned {
        let failures = vec!["profile package.json 与计划依赖集不一致：拒绝执行 npm".to_string()];
        let _ = update_stage(instance_dir, failures.clone(), false);
        return Err(failures);
    }
    if on_disk.is_empty() {
        // Nothing to materialise: promote the stage honestly.
        return update_stage(instance_dir, Vec::new(), true)
            .map(|_| 0)
            .map_err(|m| vec![m]);
    }

    // Brief §10.4: a git pin may be a SHORT commit SHA — resolve it to the
    // unique full commit before npm runs. The RESOLVED set is written to
    // package.json AND synced into the marker, so a retry after a network
    // failure never mistakes its own resolution for user drift (R2-01).
    let mut resolved = on_disk.clone();
    for (name, value) in resolved.iter_mut() {
        let Some(spec) = value.as_str() else { continue };
        let Some(rest) = spec.strip_prefix("github:") else {
            continue;
        };
        let Some((repo, pin)) = rest.rsplit_once('#') else {
            continue;
        };
        let hex_only = !pin.is_empty() && pin.chars().all(|c| c.is_ascii_hexdigit());
        if pin.len() < 40 && hex_only {
            match resolver.resolve(repo, pin).await {
                Some(full) => *value = serde_json::Value::String(format!("github:{repo}#{full}")),
                None => {
                    let failures = vec![format!(
                        "git 依赖 {name} 的短 SHA {pin} 无法解析为唯一完整 commit，已阻止"
                    )];
                    let _ = update_stage(instance_dir, failures.clone(), false);
                    return Err(failures);
                }
            }
        }
    }
    if resolved != on_disk {
        let mut updated = parsed.clone();
        if let Some(deps) = updated
            .get_mut("dependencies")
            .and_then(|d| d.as_object_mut())
        {
            *deps = resolved.clone();
        }
        write_json_atomic(
            &package_json,
            &serde_json::to_vec_pretty(&updated)
                .map_err(|e| vec![format!("无法序列化解析后的 package.json: {e}")])?,
        )?;
        // Keep the plan in step with the resolution.
        let mut sync_marker = marker.clone();
        sync_marker.planned_dependencies = resolved.clone();
        write_json_atomic(
            &marker_path,
            &serde_json::to_vec_pretty(&sync_marker).map_err(|e| vec![e.to_string()])?,
        )?;
    }

    task.set_phase("installingDeps");
    let node = match pre_resolved_node {
        Some(node) => node.to_path_buf(),
        None => crate::versions::dependencies::pick_npm_capable_node(root).ok_or_else(|| {
            vec!["未找到可用的 npm（需要 PHL 安装的 Runtime 或系统 Node）".to_string()]
        })?,
    };
    let registry_base = "https://registry.npmjs.org";
    let failures: Vec<String> = match crate::versions::dependencies::run_profile_npm_install(
        &node,
        &profile,
        registry_base,
        flag,
        on_progress,
    )
    .await
    {
        Ok(()) => Vec::new(),
        Err(e) => vec![format!("npm install 失败: {e}")],
    };

    let stage_ready = failures.is_empty();
    update_stage(instance_dir, failures.clone(), stage_ready).map_err(|e| vec![e])?;
    if failures.is_empty() {
        Ok(resolved.len())
    } else {
        Err(failures)
    }
}

/// Temp-file + rename: a crash mid-write can never truncate the marker the
/// launch gate reads (R2-02).
fn write_json_atomic(path: &Path, bytes: &[u8]) -> Result<(), Vec<String>> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| vec![format!("无法写入 {}: {e}", tmp.display())])?;
    std::fs::rename(&tmp, path).map_err(|e| vec![format!("无法原子替换 {}: {e}", path.display())])
}

/// Test hook: phase B against an installed instance without the guarded
/// command wrapper (the command path is covered by the E2E gate).
#[cfg(test)]
pub(crate) async fn prepare_dependencies_for_test(
    root: &Path,
    instance_dir: &Path,
    on_progress: &(dyn Fn(f64) + Send + Sync),
) -> Result<usize, Vec<String>> {
    let task = crate::instances::trial::tests::task();
    let flag = Arc::new(AtomicBool::new(false));
    prepare_dependencies(root, &task, &flag, instance_dir, on_progress).await
}

/// Test hook with a controlled short-SHA resolver (R2-01).
#[cfg(test)]
pub(crate) async fn prepare_dependencies_for_test_with(
    root: &Path,
    instance_dir: &Path,
    on_progress: &(dyn Fn(f64) + Send + Sync),
    resolver: &dyn ShortShaResolver,
) -> Result<usize, Vec<String>> {
    let task = crate::instances::trial::tests::task();
    let flag = Arc::new(AtomicBool::new(false));
    prepare_dependencies_with_resolver(root, &task, &flag, instance_dir, on_progress, resolver)
        .await
}

/// Resolves a short commit SHA to the unique full commit via the GitHub
/// API. `None` = network failure, unknown SHA or ambiguity — the caller
/// blocks rather than floats (brief §10.4).
///
/// Bounded by a TOTAL request timeout, not just the client's connect
/// timeout: a server that accepts the connection and then stalls the body
/// would otherwise hold the dependency-prepare task — and its instance
/// resource lock — forever, with the cancel flag never consulted inside
/// this await.
async fn resolve_short_commit_via_api(repo: &str, short: &str) -> Option<String> {
    let request = async {
        let client = crate::versions::http_client();
        let url = format!("https://api.github.com/repos/{repo}/commits/{short}");
        let value: serde_json::Value = client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        value.get("sha")?.as_str().map(str::to_string)
    };
    tokio::time::timeout(std::time::Duration::from_secs(20), request)
        .await
        .ok()
        .flatten()
}

/// Atomically rewrites the import marker's stage/failure fields.
///
/// Strictly read-modify-write: an unreadable or unparsable marker is an
/// ERROR, never a default. Phase B treats the same file as a hard failure
/// precisely so corruption can never become an empty plan (R2-02/R4-01);
/// fabricating a fresh marker here would destroy the evidence the launch
/// gate and retry card depend on and re-arm the exact hole R3-03 closed.
pub(super) fn update_stage(
    instance_dir: &Path,
    failures: Vec<String>,
    ready: bool,
) -> Result<(), String> {
    let marker_path = instance_dir.join("phl-import.json");
    let raw = std::fs::read_to_string(&marker_path)
        .map_err(|e| format!("导入记录不可读取，拒绝改写状态: {e}"))?;
    let mut marker: ImportMarker =
        serde_json::from_str(&raw).map_err(|e| format!("导入记录损坏，拒绝改写状态: {e}"))?;
    marker.dependencies_failed = failures;
    marker.stage = if ready { "ready" } else { "needsDependencies" }.into();
    // Atomic replace (R2-02): a crash mid-write can never truncate the
    // marker the launch gate reads.
    let bytes = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
    write_json_atomic(&marker_path, &bytes).map_err(|e| e.join("；"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn install_community_pack(
    transfers: State<'_, crate::versions::Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    req: CommunityInstallRequest,
    on_progress: Channel<crate::versions::ProgressEvent>,
) -> Result<CommunityInstallOutcome, String> {
    let root = phl.root().to_path_buf();
    let id = req.instance.id.clone();
    let flag = transfers.take(&transfer_id);
    let span_result = crate::operations::begin(
        &root,
        crate::operations::OperationRecord {
            operation_id: format!(
                "community-{}-{}",
                crate::versions::now_millis(),
                std::process::id()
            ),
            kind: "community-pack-install".into(),
            source_id: String::new(),
            target_id: id.clone(),
            label: format!("安装社区整合包 → 实例 {id}"),
            status: "running".into(),
            detail: {
                let mut d = serde_json::Map::new();
                d.insert(
                    "dshVersion".into(),
                    serde_json::Value::String(req.dsh_version.clone()),
                );
                d.insert(
                    "packSha256".into(),
                    serde_json::Value::String(req.pack_sha256.clone()),
                );
                d
            },
            error: None,
            started_at: String::new(),
            finished_at: None,
            retry_of: None,
        },
    );
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "community-pack-install",
        format!("安装社区整合包 → 实例 {id}"),
        vec![
            crate::resources::Resource::Instance(id.clone()),
            crate::resources::Resource::Version(req.dsh_version.clone()),
            crate::resources::Resource::Runtime(req.instance.runtime_id.clone()),
        ],
        Some(flag.clone()),
        &locks,
        &tasks,
        |task| {
            let root = root.clone();
            let on_progress = move |fraction: f64| {
                let _ = on_progress
                    .send(crate::versions::ProgressEvent::Extracting { progress: fraction });
            };
            async move { install_inner(&root, &task, &flag, req, &on_progress).await }
        },
    )
    .await;
    transfers.release(&transfer_id);
    match &result {
        Ok(outcome) => {
            if let Ok(span) = span_result {
                let mut detail = serde_json::Map::new();
                detail.insert(
                    "readiness".into(),
                    serde_json::Value::String(outcome.readiness.clone()),
                );
                detail.insert(
                    "dependenciesInstalled".into(),
                    serde_json::json!(outcome.dependencies_installed),
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
    if result.is_ok() {
        if let Ok(dir) = crate::instances::instance_dir(&root, &id) {
            crate::instances::invalidate_disk_usage(&dir);
        }
    }
    result
}

/// Retriable phase B for an instance that already exists as
/// needsDependencies: same instance, only the missing dependencies.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn prepare_pack_dependencies(
    transfers: State<'_, crate::versions::Transfers>,
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    processes: State<'_, crate::launch::Processes>,
    phl: State<'_, PhlState>,
    transfer_id: String,
    instance_id: String,
    on_progress: Channel<crate::versions::ProgressEvent>,
) -> Result<CommunityInstallOutcome, String> {
    let root = phl.root().to_path_buf();
    prepare_pack_dependencies_inner(
        &root,
        &transfers,
        &locks,
        &tasks,
        &processes,
        transfer_id,
        instance_id,
        on_progress,
    )
    .await
}

/// The retry command's body, split out so the gate itself is testable
/// (R2-06): managed community-import target, stopped, locked against the
/// runtime npm actually uses.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prepare_pack_dependencies_inner(
    root: &Path,
    transfers: &crate::versions::Transfers,
    locks: &crate::resources::ResourceLocks,
    tasks: &crate::resources::Tasks,
    processes: &crate::launch::Processes,
    transfer_id: String,
    instance_id: String,
    on_progress: Channel<crate::versions::ProgressEvent>,
) -> Result<CommunityInstallOutcome, String> {
    let root = root.to_path_buf();
    let dir = crate::instances::instance_dir(&root, &instance_id)?;
    let manifest = crate::instances::read_manifest(&dir)
        .await
        .ok_or_else(|| "实例不存在或清单不可读".to_string())?;
    // R2-06 pre-checks (inside the lock they are repeated authoritatively):
    // only managed community-import targets may rebuild dependencies.
    if manifest.management_mode == crate::instances::ManagementMode::External {
        return Err("原地接入的实例不受 PHL 管理，不能重建依赖".into());
    }
    if !crate::instances::read_import_state(&dir, Some(&manifest))
        .await
        .is_import_target()
    {
        return Err("该实例不是社区整合包导入目标（无导入记录），不能执行依赖安装".into());
    }
    // The runtime the npm phase will actually use: the instance's own when
    // it can run npm, otherwise the general pick — locked either way so a
    // concurrent runtime removal cannot interleave (R2-06).
    let npm_node = npm_node_for(&root, &manifest.runtime_id);
    let npm_runtime = npm_node
        .as_deref()
        .and_then(|node| runtime_name_of_node(&root, node))
        .unwrap_or_else(|| manifest.runtime_id.clone());

    let flag = transfers.take(&transfer_id);
    let result = crate::resources::guarded(
        transfer_id.clone(),
        "pack-dependencies",
        format!("安装整合包依赖 {instance_id}"),
        vec![
            crate::resources::Resource::Instance(instance_id.clone()),
            crate::resources::Resource::Version(
                manifest
                    .version_id
                    .strip_prefix("dsh-")
                    .unwrap_or(&manifest.version_id)
                    .to_string(),
            ),
            crate::resources::Resource::Runtime(manifest.runtime_id.clone()),
            crate::resources::Resource::Runtime(npm_runtime.clone()),
        ],
        Some(flag.clone()),
        locks,
        tasks,
        |task| {
            let root = root.clone();
            let instance_id_owned = instance_id.clone();
            let npm_node = npm_node.clone();
            let on_progress = move |fraction: f64| {
                let _ = on_progress
                    .send(crate::versions::ProgressEvent::Extracting { progress: fraction });
            };
            async move {
                // Authoritative re-checks INSIDE the lock (R2-06): a launch
                // that slipped in before locking, or a source that stopped
                // being a community import, refuses here.
                crate::instances::snapshot::ensure_not_running(processes, &instance_id_owned)?;
                let fresh = crate::instances::read_manifest(&dir)
                    .await
                    .ok_or_else(|| "实例清单在操作期间不可读".to_string())?;
                if fresh.management_mode == crate::instances::ManagementMode::External {
                    return Err("原地接入的实例不受 PHL 管理，不能重建依赖".into());
                }
                if !crate::instances::read_import_state(&dir, Some(&fresh))
                    .await
                    .is_import_target()
                {
                    return Err("该实例不是社区整合包导入目标，不能执行依赖安装".into());
                }
                Ok(
                    match prepare_dependencies_with_node(
                        &root,
                        &task,
                        &flag,
                        &dir,
                        &on_progress,
                        npm_node.as_deref(),
                    )
                    .await
                    {
                        Ok(count) => CommunityInstallOutcome {
                            record: crate::instances::build_record(&dir, fresh).await,
                            readiness: "readyToLaunch".into(),
                            dependencies_installed: count,
                            dependency_failures: Vec::new(),
                            sessions_imported: 0,
                        },
                        Err(failures) => CommunityInstallOutcome {
                            record: crate::instances::build_record(&dir, fresh).await,
                            readiness: "needsDependencies".into(),
                            dependencies_installed: 0,
                            dependency_failures: failures,
                            sessions_imported: 0,
                        },
                    },
                )
            }
        },
    )
    .await;
    transfers.release(&transfer_id);
    result
}

/// The node program the npm phase should use: the instance's bound runtime
/// when it ships npm, the general pick otherwise (R2-06).
fn npm_node_for(root: &Path, runtime_id: &str) -> Option<PathBuf> {
    if runtime_id != "node-system" {
        let bin =
            crate::launch::runtime_bin_dir(root, runtime_id).join(crate::launch::node_binary());
        if bin.exists() && crate::versions::dependencies::find_npm_cli(&bin).is_some() {
            return Some(bin);
        }
    }
    crate::versions::dependencies::pick_npm_capable_node(root)
}

/// The runtime directory name a node program lives under, when it is one.
fn runtime_name_of_node(root: &Path, node: &Path) -> Option<String> {
    let runtimes = root.join("runtimes");
    let rel = node.strip_prefix(&runtimes).ok()?;
    rel.components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
}

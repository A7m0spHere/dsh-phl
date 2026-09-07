//! Local DSH discovery: find the DSH environments already on this machine.
//!
//! The product question this answers (development spec part 1): a user has
//! used DSH for a long time and PHL should not ask them to start over. A
//! scan surfaces every DSH_HOME it can find — the `$DSH_HOME` override, the
//! default `~/.dsh`, and DSH commands on PATH plus the platform module's
//! well-known bin roots (macOS/Linux GUI launches do not inherit the shell
//! PATH, so npm/Homebrew installs need explicit probing) — plus manual
//! picks through the dialogs. Each hit is *inspected*, not assumed:
//! the markers, version reads and counters follow
//! `docs/research/dsh-session-integration.md`, which verified them against
//! real installs.
//!
//! Boundaries that define this module:
//! - read-only. Discovery never writes into a foreign home; the mutation
//!   question belongs to adoption (`instances::adoption`).
//! - no whole-disk scanning, no zstd decoding, no executing the discovered
//!   command. A PATH file-existence probe and a bounded `node --version` are
//!   the only subprocess activity from here.
//! - homes that are already PHL instance directories come back marked
//!   `alreadyManaged` so the UI can explain rather than offer to re-adopt;
//!   adoption re-checks and refuses regardless of what the scan reported.

pub(crate) mod candidate;
pub(crate) mod inspect;

// Platform halves of spec §2.1's layout. Each provides the command-name set
// and the extra bin roots a GUI-launched app cannot get from its PATH; the
// shared inspection code (`inspect.rs`) has no platform branches of its own.
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(windows)]
pub(crate) mod windows;

// The other two platform files are pure portable code (std + `dirs`), so test
// builds include them regardless of host. This keeps them compile-checked and
// unit-tested on every OS instead of being dead, unverified code until
// someone first builds on the target platform (the L-12 gate).
#[cfg(all(test, not(target_os = "linux")))]
#[path = "linux.rs"]
#[allow(dead_code)]
mod linux_check;
#[cfg(all(test, not(target_os = "macos")))]
#[path = "macos.rs"]
#[allow(dead_code)]
mod macos_check;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use tauri::State;

pub use candidate::{CandidateSource, Confidence, DshCandidate, ManagedInstance};

use crate::instances::{instances_root, read_manifest};
use crate::paths::{ensure_under_root, PhlState};
use candidate::{candidate_id, home_key};

/// `node` probed once per scan: spawning `node --version` per candidate
/// would multiply the cost of the slowest step for no extra information.
pub(crate) struct NodeProbe {
    path: Option<String>,
    version: Option<String>,
}

impl NodeProbe {
    fn probe() -> Self {
        let (path, version) = inspect::probe_node();
        Self { path, version }
    }
}

/// Assemble the candidate for one inspected home. Everything observable is
/// read here; the `alreadyManaged` overlay is applied by the caller so the
/// same builder serves scans and manual inspection.
pub(crate) fn build_candidate(
    home: &Path,
    source: CandidateSource,
    executable: Option<&Path>,
    node: &NodeProbe,
) -> DshCandidate {
    let confidence = inspect::classify_home(home);
    let mut warnings: Vec<String> = Vec::new();

    let profile_choice = match inspect::pick_profile(home) {
        Some(pair) => Some(pair),
        None => {
            warnings.push("未找到任何 profile 目录".into());
            None
        }
    };
    let profile_name = profile_choice.as_ref().map(|(n, _)| n.clone());
    let profile_dir = profile_choice.map(|(_, d)| d);

    let mut plugin_count = 0;
    if let Some(dir) = &profile_dir {
        match inspect::declared_plugins(dir) {
            Ok((count, plugin_warnings)) => {
                plugin_count = count;
                warnings.extend(plugin_warnings);
            }
            Err(e) => warnings.push(e),
        }
    }

    let session_count = inspect::count_sessions(home);
    let detected_version = inspect::detect_version(home, profile_dir.as_deref());
    if detected_version.is_none() {
        warnings.push("无法从 profile 包中确定 DSH 版本".into());
    }

    match confidence {
        Confidence::Medium => {
            warnings.push("目录结构不完整：settings.yaml 与 profiles 仅存在其一".into())
        }
        Confidence::Low => warnings.push("仅有部分 DSH 痕迹，接入前请确认这是预期的环境".into()),
        Confidence::Invalid => warnings.push("这不是一个 DSH 数据目录".into()),
        Confidence::High => {}
    }

    let display_name = if source == CandidateSource::DefaultHome {
        inspect::symbolic_display(home).unwrap_or_else(|| dir_label(home))
    } else {
        dir_label(home)
    };

    DshCandidate {
        id: candidate_id(home),
        display_name,
        dsh_home: home.to_string_lossy().into_owned(),
        executable_path: executable.map(|p| p.to_string_lossy().into_owned()),
        detected_version,
        node_path: node.path.clone(),
        node_version: node.version.clone(),
        profile: profile_name.unwrap_or_else(|| "web".into()),
        plugin_count,
        session_count,
        size_bytes: if confidence == Confidence::Invalid {
            0
        } else {
            inspect::tree_size(home)
        },
        source,
        confidence,
        warnings,
        already_managed: false,
        managed_instance: None,
    }
}

fn dir_label(home: &Path) -> String {
    home.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| home.to_string_lossy().into_owned())
}

/// Map every PHL-managed instance's `dsh-home` to its owner. Instances whose
/// manifest cannot be read are skipped: an unreadable instance is the
/// instance list's problem, and discovery must not claim it manages homes.
async fn managed_homes(root: &Path) -> HashMap<String, ManagedInstance> {
    let mut out = HashMap::new();
    let base = instances_root(root);
    let Ok(entries) = std::fs::read_dir(&base) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(manifest) = read_manifest(&dir).await else {
            continue;
        };
        // The instance's ACTUAL home via `home_of`, not `dir/dsh-home`: an
        // external instance's home lives outside its directory, and this map
        // is what flags a candidate `alreadyManaged` and what adoption re-checks
        // to refuse a second owner of one DSH_HOME (spec §1.1). Keying the
        // wrong path here would let both guards miss every external instance.
        out.insert(
            home_key(&crate::instances::home_of(&dir, &manifest)),
            ManagedInstance {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
            },
        );
    }
    out
}

/// Attach the `alreadyManaged` overlay. A home inside the PHL data root that
/// matches no instance is also called out: adoption must not treat stray
/// trees under `instances/` as external environments.
fn mark_managed(
    candidate: &mut DshCandidate,
    managed: &HashMap<String, ManagedInstance>,
    phl_root: &Path,
) {
    let home = PathBuf::from(&candidate.dsh_home);
    if let Some(owner) = managed.get(&home_key(&home)) {
        candidate.already_managed = true;
        candidate.managed_instance = Some(owner.clone());
        candidate
            .warnings
            .push(format!("该环境已由 PHL 实例「{}」管理", owner.name));
        return;
    }
    let inside_instances =
        ensure_under_root(phl_root, &home).is_ok() && home.starts_with(instances_root(phl_root));
    if inside_instances {
        candidate
            .warnings
            .push("该目录位于 PHL 数据目录内，但不是任何在册实例的 DSH_HOME".into());
    }
}

/// Order candidates for display: strongest evidence first, ties by path so
/// rescans never reshuffle the list under the user's selection.
fn rank_candidate(candidate: &DshCandidate) -> u8 {
    match candidate.confidence {
        Confidence::High => 3,
        Confidence::Medium => 2,
        Confidence::Low => 1,
        Confidence::Invalid => 0,
    }
}

/// Environment-derived inputs to a scan. Every process-spawning or env-reading
/// step happens when gathering these; the scan core is then pure over them, so
/// tests can run a full scan against fixture roots without ever reading the
/// real `~/.dsh`, PATH, or spawning `node`.
pub(crate) struct ScanInputs {
    /// Candidate DSH_HOMEs discovered from `$DSH_HOME` and the default home.
    pub(crate) homes: Vec<(PathBuf, CandidateSource)>,
    /// The home DSH would resolve with no data yet (`$DSH_HOME` > `~/.dsh`) —
    /// the anchor for a PATH-install candidate.
    pub(crate) default_home: PathBuf,
    /// A DSH command located on PATH, if any.
    pub(crate) executable: Option<PathBuf>,
    pub(crate) node: NodeProbe,
}

impl ScanInputs {
    /// Probe the live machine. Called by the real commands, never by tests.
    async fn gather() -> Result<Self, String> {
        tokio::task::spawn_blocking(|| {
            let homes = inspect::default_homes();
            let default_home = homes
                .iter()
                .find(|(_, source)| {
                    !matches!(source, CandidateSource::Path | CandidateSource::Manual)
                })
                .map(|(p, _)| p.clone())
                .or_else(|| dirs::home_dir().map(|h| h.join(".dsh")))
                .unwrap_or_default();
            ScanInputs {
                homes,
                default_home,
                executable: inspect::find_dsh_executable(),
                node: NodeProbe::probe(),
            }
        })
        .await
        .map_err(|e| format!("读取本机环境失败: {e}"))
    }
}

/// Turn a set of roots + environment inputs into sorted, deduplicated,
/// managed-marked candidates. Synchronous: filesystem walks and the (pure)
/// classification. `manual` roots outrank automatic ones on path collision —
/// the directory the user named always wins provenance. Invalid roots are
/// dropped at the end: they are not DSH environments, and the manual command
/// turns an empty result into its own "that path is not a DSH home" error.
fn scan_core(
    manual: Vec<(PathBuf, CandidateSource)>,
    inputs: &ScanInputs,
    managed: &HashMap<String, ManagedInstance>,
    phl_root: &Path,
) -> Vec<DshCandidate> {
    let mut out: Vec<DshCandidate> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let manual_keys: Vec<String> = manual.iter().map(|(p, _)| home_key(p)).collect();

    for (path, source) in manual {
        seen.insert(home_key(&path));
        out.push(build_candidate(
            &path,
            source,
            inputs.executable.as_deref(),
            &inputs.node,
        ));
    }
    for (path, source) in &inputs.homes {
        if manual_keys.contains(&home_key(path)) || !seen.insert(home_key(path)) {
            continue;
        }
        out.push(build_candidate(
            path,
            *source,
            inputs.executable.as_deref(),
            &inputs.node,
        ));
    }

    // A DSH command on PATH with no home that survived the scan (never
    // launched, or a `$DSH_HOME` we could not read): the install exists, so
    // surface it as a Path-sourced candidate anchored on the default home.
    // Promotion (not a fresh push) is what keeps the dedup honest: when
    // `~/.dsh` was already built above as Invalid, we rewrite that row in
    // place instead of colliding on its key.
    if let Some(exe) = &inputs.executable {
        let key = home_key(&inputs.default_home);
        if !manual_keys.contains(&key) {
            let valid_elsewhere = out.iter().any(|c| {
                c.confidence != Confidence::Invalid && home_key(Path::new(&c.dsh_home)) == key
            });
            if !valid_elsewhere {
                if let Some(existing) = out
                    .iter_mut()
                    .find(|c| home_key(Path::new(&c.dsh_home)) == key)
                {
                    promote_to_path_candidate(existing, exe);
                } else {
                    let mut candidate = build_candidate(
                        &inputs.default_home,
                        CandidateSource::Path,
                        Some(exe),
                        &inputs.node,
                    );
                    promote_to_path_candidate(&mut candidate, exe);
                    out.push(candidate);
                }
            }
        }
    }

    out.retain(|c| c.confidence != Confidence::Invalid);
    out.sort_by(|a, b| {
        rank_candidate(b)
            .cmp(&rank_candidate(a))
            .then_with(|| a.dsh_home.cmp(&b.dsh_home))
    });
    for candidate in out.iter_mut() {
        mark_managed(candidate, managed, phl_root);
    }
    out
}

/// A Path candidate is "installed, data home not yet created". An absent home
/// classified `Invalid`, but the located command is installation evidence, so
/// the row is promoted to `Low` and the misleading "not a DSH" warning dropped.
fn promote_to_path_candidate(candidate: &mut DshCandidate, _exe: &Path) {
    if candidate.confidence == Confidence::Invalid {
        candidate.confidence = Confidence::Low;
        candidate
            .warnings
            .retain(|w| !w.contains("这不是一个 DSH 数据目录"));
        candidate.size_bytes = 0;
    }
    candidate.source = CandidateSource::Path;
    if !candidate
        .warnings
        .iter()
        .any(|w| w.contains("仅发现 PATH 上的 DSH 命令"))
    {
        candidate
            .warnings
            .push("仅发现 PATH 上的 DSH 命令，数据目录尚未创建（首次启动后会出现历史对话）".into());
    }
}

/// The instance that manages the home identified by `home_key`, if any.
/// Adoption re-checks this against the live root before committing — the
/// `alreadyManaged` flag from a scan is advisory (it can be stale), the
/// refusal at commit is authoritative.
pub(crate) async fn managed_instance_for(
    root: &Path,
    key: &str,
) -> Option<candidate::ManagedInstance> {
    managed_homes(root).await.get(key).cloned()
}

/* ------------------------------- commands ------------------------------- */

/// Rescan the machine for DSH environments. Read-only and lock-free: nothing
/// here mutates, so there is nothing for the resource registry to protect
/// (adoption takes its locks when the user commits).
#[tauri::command]
pub async fn discover_dsh(state: State<'_, PhlState>) -> Result<Vec<DshCandidate>, String> {
    let root = state.root();
    let inputs = ScanInputs::gather().await?;
    let managed = managed_homes(&root).await;
    tokio::task::spawn_blocking(move || scan_core(Vec::new(), &inputs, &managed, &root))
        .await
        .map_err(|e| format!("发现扫描失败: {e}"))
}

/// Inspect one manually chosen DSH_HOME. A dead choice is an error the dialog
/// can show — the user aimed at a specific directory, and "that is not a DSH
/// home" is the finding, with the confidence trail behind it. This does NOT
/// scan the rest of the machine: it builds exactly the one candidate.
#[tauri::command]
pub async fn inspect_dsh_home(
    state: State<'_, PhlState>,
    path: String,
) -> Result<DshCandidate, String> {
    let home = PathBuf::from(path.trim());
    if home.as_os_str().is_empty() {
        return Err("未选择 DSH 目录".into());
    }
    if !home.is_dir() {
        return Err(format!("目录不存在: {}", home.display()));
    }
    let root = state.root();
    let managed = managed_homes(&root).await;
    let candidate = tokio::task::spawn_blocking(move || {
        let executable = inspect::find_dsh_executable();
        let candidate = build_candidate(
            &home,
            CandidateSource::Manual,
            executable.as_deref(),
            &NodeProbe::probe(),
        );
        let mut candidate = candidate;
        mark_managed(&mut candidate, &managed, &root);
        candidate
    })
    .await
    .map_err(|e| format!("检查 DSH 目录失败: {e}"))?;
    if candidate.confidence == Confidence::Invalid {
        return Err(format!(
            "该目录不是可接入的 DSH 数据目录（需要 settings.yaml 或 profiles/）: {}",
            candidate.dsh_home
        ));
    }
    Ok(candidate)
}

/// Inspect a manually chosen DSH executable. The command names the install;
/// the home is whatever DSH itself would resolve when that command runs
/// (`$DSH_HOME` > `~/.dsh`, spike §1), recorded as a *prediction* the adoption
/// preview re-verifies on screen.
#[tauri::command]
pub async fn inspect_dsh_executable(
    state: State<'_, PhlState>,
    path: String,
) -> Result<DshCandidate, String> {
    let exe = PathBuf::from(path.trim());
    if exe.as_os_str().is_empty() {
        return Err("未选择 DSH 可执行文件".into());
    }
    if !exe.is_file() {
        return Err(format!("文件不存在: {}", exe.display()));
    }
    let root = state.root();
    let managed = managed_homes(&root).await;
    let candidate = tokio::task::spawn_blocking(move || {
        let inferred = inspect::default_homes().into_iter().next();
        let (home, rule) = inferred.unwrap_or_else(|| {
            (
                dirs::home_dir().unwrap_or_default().join(".dsh"),
                CandidateSource::DefaultHome,
            )
        });
        let mut candidate = build_candidate(
            &home,
            CandidateSource::Manual,
            Some(&exe),
            &NodeProbe::probe(),
        );
        // Provenance honesty: the *command* was manual, the home is inferred
        // by DSH's own resolution rule — say which one.
        candidate.warnings.insert(
            0,
            format!(
                "可执行文件为手动选择；DSH_HOME 按 {} 推断",
                home_rule_label(rule)
            ),
        );
        mark_managed(&mut candidate, &managed, &root);
        candidate
    })
    .await
    .map_err(|e| format!("检查 DSH 可执行文件失败: {e}"))?;
    Ok(candidate)
}

fn home_rule_label(source: CandidateSource) -> &'static str {
    match source {
        CandidateSource::Env => "$DSH_HOME 环境变量",
        CandidateSource::DefaultHome => "默认路径 ~/.dsh",
        CandidateSource::Path => "PATH 安装",
        CandidateSource::Manual => "默认规则",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-discover-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_home(dir: &Path) {
        std::fs::create_dir_all(dir.join("profiles").join("web").join("node_modules")).unwrap();
        std::fs::write(dir.join("settings.yaml"), "ui: {}\n").unwrap();
    }

    #[tokio::test]
    async fn ids_are_stable_across_spellings() {
        let dir = fixture("id");
        fake_home(&dir);
        let node = NodeProbe {
            path: None,
            version: None,
        };
        let a = build_candidate(&dir, CandidateSource::DefaultHome, None, &node);
        let mixed = {
            let text = dir.to_string_lossy().replace('\\', "/");
            PathBuf::from(text)
        };
        let b = build_candidate(&mixed, CandidateSource::Manual, None, &node);
        assert_eq!(a.id, b.id, "separator spelling must not fork identities");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn empty_inputs() -> ScanInputs {
        ScanInputs {
            homes: Vec::new(),
            default_home: PathBuf::new(),
            executable: None,
            node: NodeProbe {
                path: None,
                version: None,
            },
        }
    }

    #[tokio::test]
    async fn scan_drops_invalid_and_ranks_the_rest() {
        let dir = fixture("scan");
        let valid = dir.join("real");
        fake_home(&valid);
        let junk = dir.join("junk");
        std::fs::create_dir_all(junk.join("stuff")).unwrap();
        let inputs = empty_inputs();
        let candidates = scan_core(
            vec![
                (valid.clone(), CandidateSource::Manual),
                (junk.clone(), CandidateSource::Manual),
            ],
            &inputs,
            &HashMap::new(),
            &dir,
        );
        assert_eq!(candidates.len(), 1, "invalid manual roots are dropped");
        assert_eq!(Path::new(&candidates[0].dsh_home), valid.as_path());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn manual_root_wins_over_auto_duplicate() {
        let dir = fixture("dup");
        let home = dir.join("env-home");
        fake_home(&home);
        let mut inputs = empty_inputs();
        inputs.homes = vec![(home.clone(), CandidateSource::Env)];
        // Manual names the same directory; only one candidate, Manual source.
        let candidates = scan_core(
            vec![(home.clone(), CandidateSource::Manual)],
            &inputs,
            &HashMap::new(),
            &dir,
        );
        assert_eq!(candidates.len(), 1, "dedup collapses the same home");
        assert_eq!(candidates[0].source, CandidateSource::Manual);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn path_executable_synthesizes_a_candidate_when_no_home_survives() {
        let dir = fixture("path-synth");
        let exe = dir.join("dsh.cmd");
        std::fs::write(&exe, "@echo off").unwrap();
        // No homes at all: a PATH command still means "DSH is installed",
        // anchored on the (absent) default home DSH would create.
        let mut inputs = empty_inputs();
        inputs.default_home = dir.join(".dsh");
        inputs.executable = Some(exe.clone());
        let candidates = scan_core(Vec::new(), &inputs, &HashMap::new(), &dir);
        assert_eq!(
            candidates.len(),
            1,
            "PATH-only install surfaces a candidate"
        );
        assert_eq!(candidates[0].source, CandidateSource::Path);
        assert_eq!(candidates[0].confidence, Confidence::Low);
        assert_eq!(
            Path::new(&candidates[0].dsh_home),
            dir.join(".dsh").as_path()
        );
        assert_eq!(
            candidates[0].executable_path.as_deref(),
            Some(exe.to_string_lossy().as_ref())
        );
        assert!(candidates[0]
            .warnings
            .iter()
            .any(|w| w.contains("仅发现 PATH")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn managed_overlay_marks_the_owning_instance() {
        let dir = fixture("managed");
        let inst = dir.join("instances").join("abc-1");
        std::fs::create_dir_all(inst.join("dsh-home")).unwrap();
        let manifest = crate::instances::InstanceManifest {
            schema_version: crate::instances::manifest::MANIFEST_SCHEMA_VERSION,
            id: "abc-1".into(),
            name: "我的实例".into(),
            note: None,
            kind: "sandbox".into(),
            hue: 210,
            version_id: "0.1.2-rc.1".into(),
            runtime_id: "node-22".into(),
            port: 6080,
            auto_port: true,
            profile: "web".into(),
            created_at: crate::versions::now_iso(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: Default::default(),
            args: Vec::new(),
            api: None,
            management_mode: Default::default(),
            source: Default::default(),
            external_home: None,
            adopted_from: None,
        };
        crate::instances::manifest::write_manifest(&inst, &manifest)
            .await
            .unwrap();
        let managed = managed_homes(&dir).await;
        let home = inst.join("dsh-home");
        let node = NodeProbe {
            path: None,
            version: None,
        };
        fake_home(&home);
        let mut candidate = build_candidate(&home, CandidateSource::DefaultHome, None, &node);
        mark_managed(&mut candidate, &managed, &dir);
        assert!(candidate.already_managed);
        assert_eq!(candidate.managed_instance.as_ref().unwrap().id, "abc-1");

        // A stray directory under instances/ that is not a registered home.
        let stray = dir.join("instances").join("stray").join("dsh-home");
        std::fs::create_dir_all(&stray).unwrap();
        fake_home(&stray);
        let mut stray_candidate = build_candidate(&stray, CandidateSource::Manual, None, &node);
        mark_managed(&mut stray_candidate, &managed, &dir);
        assert!(!stray_candidate.already_managed);
        assert!(stray_candidate
            .warnings
            .iter()
            .any(|w| w.contains("不是任何在册实例")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

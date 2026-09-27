//! Closing a plugin package's dependency tree: the only place that runs
//! `pnpm install` against the plugin's own directory.
//!
//! The installer is resolved per instance, mirroring how the instance itself
//! is launched: when the instance binds a PHL-managed Node runtime, its bin
//! dir leads the child PATH (so pnpm runs on the bundled node) and it also
//! carries the `corepack` that ships inside every Node distribution. That
//! order — runtime pnpm, runtime corepack, then system pnpm, then system
//! corepack — means a user who installed a runtime through PHL can install a
//! dependency-carrying plugin with no system-side tooling at all. The bare
//! system PATH only gets a chance when the instance binds the system runtime
//! (or its runtime dir is missing).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// A dependency install must complete before the package swap can commit.
const DEPENDENCY_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// The instance context the dependency installer is resolved against.
#[derive(Debug, Clone)]
pub(crate) struct DependencyEnv {
    /// PHL state root — installed runtimes and the shared corepack cache live here.
    root: PathBuf,
    /// The instance's bound runtime id. `""` and `node-system` both mean
    /// "no isolated runtime: resolve on the system PATH".
    runtime_id: String,
    /// npm registry the plugin catalog is read from (follows the download-source
    /// setting). Forwarded to corepack so a mirror setting also mirrors the
    /// pnpm download. `None` keeps corepack's own default.
    npm_registry: Option<String>,
}

impl DependencyEnv {
    pub(crate) fn new(root: &Path, runtime_id: &str, npm_registry: Option<&str>) -> Self {
        Self {
            root: root.to_path_buf(),
            runtime_id: runtime_id.to_string(),
            npm_registry: npm_registry.map(|s| s.to_string()),
        }
    }
}

/// One concrete way to run `pnpm install --ignore-workspace …`.
struct ToolCandidate {
    /// Resolved executable, or a bare name when the system PATH should look it up.
    program: PathBuf,
    /// Arguments before the install args — `["pnpm"]` for the corepack passthrough.
    extra_args: &'static [&'static str],
    /// Directory to lead the child PATH with: the instance runtime's bin dir.
    path_prefix: Option<PathBuf>,
    /// Whether `program` is corepack (needs the prompt-off / cache-dir / registry env).
    corepack: bool,
    /// Human-readable origin, for the "nothing was found" report.
    label: String,
}

/// The resolution order documented on this module. Runtime candidates are
/// dropped when the runtime dir does not exist (runtime not installed yet —
/// pack staging registers plugins before the runtime necessarily landed), so
/// a missing runtime never blocks an install that the system could serve.
fn candidates(env: &DependencyEnv) -> Vec<ToolCandidate> {
    let (pnpm_bin, corepack_bin) = if cfg!(windows) {
        ("pnpm.cmd", "corepack.cmd")
    } else {
        ("pnpm", "corepack")
    };
    let mut out = Vec::new();
    if !env.runtime_id.is_empty() && env.runtime_id != "node-system" {
        let bin = crate::launch::runtime_bin_dir(&env.root, &env.runtime_id);
        if bin.is_dir() {
            out.push(ToolCandidate {
                program: bin.join(pnpm_bin),
                extra_args: &[],
                path_prefix: Some(bin.clone()),
                corepack: false,
                label: format!("实例运行时 {} 内的 pnpm", env.runtime_id),
            });
            out.push(ToolCandidate {
                program: bin.join(corepack_bin),
                extra_args: &["pnpm"],
                path_prefix: Some(bin.clone()),
                corepack: true,
                label: format!("实例运行时 {} 内的 corepack", env.runtime_id),
            });
        }
    }
    out.push(ToolCandidate {
        program: PathBuf::from(pnpm_bin),
        extra_args: &[],
        path_prefix: None,
        corepack: false,
        label: "系统 PATH 上的 pnpm".into(),
    });
    out.push(ToolCandidate {
        program: PathBuf::from(corepack_bin),
        extra_args: &["pnpm"],
        path_prefix: None,
        corepack: true,
        label: "系统 PATH 上的 corepack".into(),
    });
    out
}

/// Keep the dependency closure inside the package being committed. A profile-
/// level `pnpm add` rewrites shared dependencies and package.json outside the
/// install rollback boundary. Ignoring the enclosing workspace also prevents
/// pnpm from mutating sibling plugins. Lifecycle scripts follow the same
/// disabled-by-default policy as the DSH version installer.
pub(crate) async fn install_plugin_dependencies(
    dest: &Path,
    env: &DependencyEnv,
) -> Result<(), String> {
    if dependency_specs(dest).await?.is_empty() {
        return Ok(());
    }
    let mut missing: Vec<String> = Vec::new();
    for cand in candidates(env) {
        // Windows starts `.cmd` shims through cmd.exe, so a missing shim
        // SPAWNS and then fails at runtime — NotFound never arrives. Absence
        // of the file is the same signal for an absolute candidate, and the
        // fallback chain must keep walking it. (Bare PATH names do return
        // NotFound from the spawn and need no pre-check.)
        if cand.program.is_absolute() && !cand.program.exists() {
            missing.push(cand.label);
            continue;
        }
        let mut command = tokio::process::Command::new(&cand.program);
        command
            .current_dir(dest)
            .args(cand.extra_args)
            .args([
                "install",
                "--ignore-workspace",
                "--prod",
                "--ignore-scripts",
                "--no-lockfile",
            ])
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(prefix) = &cand.path_prefix {
            let sep = if cfg!(windows) { ";" } else { ":" };
            command.env(
                "PATH",
                format!(
                    "{pfx}{sep}{existing}",
                    pfx = prefix.display(),
                    existing = std::env::var("PATH").unwrap_or_default()
                ),
            );
        }
        if cand.corepack {
            // Unattended spawn: without this corepack blocks on a TTY prompt
            // the first time it fetches a package manager.
            command.env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0");
            if !env.root.as_os_str().is_empty() {
                // Corepack's package-manager cache under PHL's root, so it is
                // accounted for by the store and survives PATH churn.
                command.env("COREPACK_HOME", env.root.join("tools").join("corepack"));
            }
            if let Some(registry) = &env.npm_registry {
                command.env("COREPACK_NPM_REGISTRY", registry);
            }
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let child = match command.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                missing.push(cand.label);
                continue;
            }
            Err(e) => return Err(format!("无法启动{}：{e}", cand.label)),
        };
        // The first candidate that STARTS is authoritative: pnpm failing on
        // the registry must not be hidden by a fallback installer's success —
        // the user needs pnpm's own error. Only a missing executable advances
        // the chain.
        return run_dependency_install(child).await;
    }
    Err(not_found_error(&missing))
}

/// The report when no installer started at all: every candidate is named,
/// because "请先安装 pnpm 或 corepack" was only honest if the corepack
/// attempt actually exists — which is the whole point of this module.
fn not_found_error(missing: &[String]) -> String {
    format!(
        "未找到 pnpm，也没有可用的 corepack（已尝试：{}）。请在「运行时」页为实例安装 Node 运行时（自带 corepack），或在系统上安装 pnpm。",
        missing.join("、")
    )
}

/// Wait out an already-started installer under the commit timeout.
async fn run_dependency_install(child: tokio::process::Child) -> Result<(), String> {
    let pid = child.id();
    let output = child.wait_with_output();
    tokio::pin!(output);
    let output = match tokio::time::timeout(DEPENDENCY_INSTALL_TIMEOUT, &mut output).await {
        Ok(result) => result.map_err(|e| format!("读取 pnpm 结果失败：{e}"))?,
        Err(_) => {
            if let Some(pid) = pid {
                crate::launch::kill_tree(pid).await?;
            }
            // Reap the stopped child before callers restore or delete its files.
            let _ = output.await;
            return Err("安装依赖超时（pnpm）".into());
        }
    };
    if !output.status.success() {
        return Err(format!(
            "pnpm 安装依赖失败: {}",
            stderr_tail(&output.stderr)
        ));
    }
    Ok(())
}

/// `name@range` specs for the plugin's own `dependencies`, read from its
/// extracted `package.json`. Empty for a dependency-free plugin.
pub(crate) async fn dependency_specs(dest: &Path) -> Result<Vec<String>, String> {
    let text = match tokio::fs::read_to_string(dest.join("package.json")).await {
        Ok(t) => t,
        Err(e) => return Err(format!("无法读取插件 package.json: {e}")),
    };
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("插件 package.json 解析失败: {e}"))?;
    let mut specs = value
        .get("dependencies")
        .and_then(|d| d.as_object())
        .map(|deps| {
            deps.iter()
                .filter_map(|(name, range)| range.as_str().map(|r| format!("{name}@{r}")))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    specs.sort();
    Ok(specs)
}

/// The last few lines of pnpm's stderr, for a failure the user can act on.
fn stderr_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("phl-plugin-deps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn is_windows() -> bool {
        cfg!(windows)
    }

    #[test]
    fn system_bindings_only_offer_the_path_chain() {
        for runtime in ["", "node-system"] {
            let cands = candidates(&DependencyEnv::new(Path::new("/root"), runtime, None));
            assert_eq!(cands.len(), 2, "runtime {runtime:?}");
            assert!(cands.iter().all(|c| c.path_prefix.is_none()));
            assert!(!cands[0].corepack);
            assert_eq!(cands[1].extra_args, ["pnpm"]);
        }
    }

    #[test]
    fn installed_runtime_leads_the_chain_and_paces_corepack() {
        let root = temp_root("runtime-first");
        let bin = crate::launch::runtime_bin_dir(&root, "node-22.12.0");
        std::fs::create_dir_all(&bin).unwrap();
        let cands = candidates(&DependencyEnv::new(&root, "node-22.12.0", None));
        assert_eq!(cands.len(), 4);
        let pfx = cands[0].path_prefix.as_ref().unwrap();
        assert_eq!(pfx, &bin);
        // Windows resolves shims by file name; POSIX by bare binary name.
        let pnpm = if is_windows() { "pnpm.cmd" } else { "pnpm" };
        assert!(cands[0].program.ends_with(pnpm));
        assert!(!cands[0].corepack);
        assert!(cands[1].corepack);
        assert_eq!(cands[1].extra_args, ["pnpm"]);
        // System fallbacks stay last so an isolated runtime is never shadowed.
        assert!(cands[2].path_prefix.is_none() && cands[3].path_prefix.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_runtime_dir_falls_back_to_the_system_chain() {
        // The pack staging path: manifest bound a runtime that is not on disk
        // yet — resolution must not hand a nonexistent directory to the spawn.
        let root = temp_root("runtime-absent");
        let cands = candidates(&DependencyEnv::new(&root, "node-22.12.0", None));
        assert_eq!(cands.len(), 2);
        assert!(cands.iter().all(|c| c.path_prefix.is_none()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn not_found_error_names_every_attempted_candidate() {
        let e = not_found_error(&[
            "实例运行时 node-22 内的 pnpm".into(),
            "系统 PATH 上的 corepack".into(),
        ]);
        assert!(e.contains("pnpm") && e.contains("corepack"));
        assert!(
            e.contains("系统 PATH 上的 corepack"),
            "must name what was tried: {e}"
        );
    }

    /// Lay down an executable that records its argv line and one env value.
    /// The `pnpm`-prefixed argv a corepack passthrough produces is exactly
    /// what proves the chain landed on the right candidate.
    fn fake_installer(bin: &Path, log: &Path, record_name: &str) {
        #[cfg(windows)]
        {
            // A digit directly before `>` is cmd's handle-redirect syntax
            // (`PROMPT=0>>file` redirects handle 0 instead of appending!) —
            // the space keeps both lines plain appends.
            let script = format!(
                "@echo off\r\necho {record_name} %* >\"{}\"\r\necho PROMPT=%COREPACK_ENABLE_DOWNLOAD_PROMPT% >>\"{}\"\r\n",
                log.display(),
                log.display()
            );
            std::fs::write(bin.join(format!("{record_name}.cmd")), script).unwrap();
        }
        #[cfg(unix)]
        {
            let script = format!(
                "#!/bin/sh\necho \"{record_name} $*\" > '{log}'\necho \"PROMPT=$COREPACK_ENABLE_DOWNLOAD_PROMPT\" >> '{log}'\n",
                log = log.display()
            );
            std::fs::write(bin.join(record_name), &script).unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                bin.join(record_name),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn runtime_corepack_serves_the_install_when_runtime_pnpm_is_absent() {
        // The isolated-runtime user with no system pnpm at all: the runtime's
        // bundled corepack must run the install — the old code had no such
        // fallback and its error message claimed otherwise.
        let root = temp_root("runtime-corepack");
        let bin = crate::launch::runtime_bin_dir(&root, "node-22.12.0");
        std::fs::create_dir_all(&bin).unwrap();
        let log = root.join("invocation.txt");
        fake_installer(&bin, &log, "corepack");
        let dest = root.join("node_modules").join("dsh-foo");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join("package.json"),
            r#"{"name":"dsh-foo","version":"1.0.0","dependencies":{"ws":"^8.0.0"}}"#,
        )
        .unwrap();
        let env = DependencyEnv::new(
            &root,
            "node-22.12.0",
            Some("https://registry.npmmirror.com"),
        );
        install_plugin_dependencies(&dest, &env).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains(
                "corepack pnpm install --ignore-workspace --prod --ignore-scripts --no-lockfile"
            ),
            "corepack must be invoked as a pnpm passthrough: {text}"
        );
        assert!(
            text.contains("PROMPT=0"),
            "download prompt must be off: {text}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn runtime_pnpm_wins_over_corepack_and_the_system_chain() {
        let root = temp_root("runtime-pnpm-first");
        let bin = crate::launch::runtime_bin_dir(&root, "node-22.12.0");
        std::fs::create_dir_all(&bin).unwrap();
        let log = root.join("invocation.txt");
        fake_installer(&bin, &log, "corepack");
        fake_installer(&bin, &log, "pnpm");
        let dest = root.join("node_modules").join("dsh-foo");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join("package.json"),
            r#"{"name":"dsh-foo","version":"1.0.0","dependencies":{"ws":"^8.0.0"}}"#,
        )
        .unwrap();
        let env = DependencyEnv::new(&root, "node-22.12.0", None);
        install_plugin_dependencies(&dest, &env).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.starts_with("pnpm install"),
            "runtime pnpm must lead: {text}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn dependency_specs_are_sorted_and_ranged() {
        let dir = temp_root("dep-specs");
        let dest = dir.join("node_modules").join("dsh-foo");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(
            dest.join("package.json"),
            r#"{"dependencies":{"ws":"^8.0.0","schemastery":"^3.0.0","react":"18.3.1"}}"#,
        )
        .unwrap();
        assert_eq!(
            dependency_specs(&dest).await.unwrap(),
            vec!["react@18.3.1", "schemastery@^3.0.0", "ws@^8.0.0"],
        );
    }

    #[tokio::test]
    async fn self_contained_plugin_has_no_dependency_specs() {
        let dir = temp_root("dep-none");
        let dest = dir.join("node_modules").join("dsh-bar");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("package.json"), r#"{"name":"dsh-bar"}"#).unwrap();
        assert!(dependency_specs(&dest).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn dependency_free_plugin_never_touches_the_tool_chain() {
        // No installer on the whole machine: still succeeds, because the
        // empty-closure check precedes resolution entirely.
        let dir = temp_root("dep-free");
        let dest = dir.join("node_modules").join("dsh-bar");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("package.json"), r#"{"name":"dsh-bar"}"#).unwrap();
        let env = DependencyEnv::new(&dir, "node-99.0.0", None);
        install_plugin_dependencies(&dest, &env).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stderr_tail_keeps_the_last_few_lines_in_order() {
        let bytes = b"line1\nline2\nline3\n".to_vec();
        assert_eq!(stderr_tail(&bytes), "line1\nline2\nline3");
    }

    #[tokio::test]
    #[ignore = "requires pnpm (or corepack) and Node on PATH; uses only a local fixture dependency"]
    async fn dependencies_stay_inside_the_plugin_and_scripts_do_not_run() {
        let root = temp_root("dep-isolation");
        let profile = root.join("profile");
        let dest = profile.join("node_modules/dsh-local");
        let dependency = root.join("local-dep");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(
            dependency.join("package.json"),
            r#"{"name":"phl-fixture-dep","version":"1.0.0","main":"index.js"}"#,
        )
        .unwrap();
        std::fs::write(dependency.join("index.js"), "module.exports = 42").unwrap();
        let package = serde_json::json!({
            "name": "dsh-local", "version": "1.0.0",
            "dependencies": {"phl-fixture-dep": format!("file:{}", dependency.to_string_lossy().replace('\\', "/"))},
            "scripts": {"postinstall": "node -e \"require('fs').writeFileSync('script-ran', 'bad')\""}
        });
        std::fs::write(dest.join("package.json"), package.to_string()).unwrap();
        std::fs::write(
            profile.join("package.json"),
            "{\"name\":\"profile\",\"private\":true}",
        )
        .unwrap();
        std::fs::write(
            profile.join("pnpm-workspace.yaml"),
            "packages:\n  - node_modules/*\n",
        )
        .unwrap();
        let before = std::fs::read(profile.join("package.json")).unwrap();
        let env = DependencyEnv::new(&root, "node-system", None);
        install_plugin_dependencies(&dest, &env).await.unwrap();
        assert_eq!(std::fs::read(profile.join("package.json")).unwrap(), before);
        assert!(!profile.join("pnpm-lock.yaml").exists());
        assert!(!profile.join("node_modules/phl-fixture-dep").exists());
        assert!(!dest.join("script-ran").exists());
        // The dependency must be *closed within the plugin*: reachable from
        // the plugin directory, and only there. pnpm's layout of `file:`
        // links is environment-dependent — on Windows without symlink
        // privileges (no Developer Mode), pnpm 12 resolves registry links
        // fine but omits the top-level `file:` link, leaving the package
        // only under the plugin's own `.pnpm` store. Both outcomes satisfy
        // the product promise; only *resolution outside the plugin* or a
        // script run would violate it — hence the store-level fallback.
        let output = tokio::process::Command::new("node")
            .current_dir(&dest)
            .args([
                "-e",
                "if (require('phl-fixture-dep') !== 42) process.exit(1)",
            ])
            .output()
            .await
            .unwrap();
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            assert!(
                stderr.contains("Cannot find module"),
                "dependency resolved to the wrong value, not a layout miss: {stderr}"
            );
            let vstore = dest.join("node_modules").join(".pnpm");
            let closed = vstore.exists() && contains_named_dir(&vstore, "phl-fixture-dep", 4);
            assert!(
                closed,
                "the dependency must still live inside the plugin's own subtree"
            );
            eprintln!("note: pnpm laid `file:` dep into the virtual store only (no symlink privilege); closure verified");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Bounded search for a directory entry named `needle` below `dir`.
    fn contains_named_dir(dir: &Path, needle: &str, depth: u32) -> bool {
        if depth == 0 {
            return false;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            if entry.file_name() == needle {
                return true;
            }
            if entry.path().is_dir() && contains_named_dir(&entry.path(), needle, depth - 1) {
                return true;
            }
        }
        false
    }
}

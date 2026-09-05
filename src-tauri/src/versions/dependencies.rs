//! Dependency installation for a DSH version: the npm solve, the mirror→
//! official 404 fallback and the experimental-only prune policy (T-005).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::now_iso;

/* --------------------------- dependency install -------------------------- */

/// An npm tarball contains the package *only* — dependencies never do. DSH's
/// `lib/bin.js` ESM-imports its own runtime deps (`@deepseek-ai/dsh-app-boot`
/// and friends), and bare-specifier resolution walks up from
/// `versions/<name>/`, so the only tree they can ever be found in is the
/// version's own `node_modules`. Extraction alone produces a version that
/// cannot boot.
pub(crate) fn package_requires_deps(version_dir: &Path) -> bool {
    #[derive(Deserialize)]
    struct PackageJson {
        #[serde(default)]
        dependencies: Option<HashMap<String, serde_json::Value>>,
        #[serde(default, rename = "peerDependencies")]
        peer_dependencies: Option<HashMap<String, serde_json::Value>>,
    }
    let Ok(raw) = std::fs::read_to_string(version_dir.join("package.json")) else {
        return false;
    };
    match serde_json::from_str::<PackageJson>(&raw) {
        Ok(pkg) => {
            !pkg.dependencies.unwrap_or_default().is_empty()
                || !pkg.peer_dependencies.unwrap_or_default().is_empty()
        }
        Err(_) => false,
    }
}

/// The launch-time repair probe for versions installed before the dependency
/// step existed (and for node_modules the user deleted).
pub(crate) fn version_deps_missing(version_dir: &Path) -> bool {
    package_requires_deps(version_dir) && !version_dir.join("node_modules").exists()
}

/// The npm CLI bundled with a Node distribution — not a globally installed
/// `npm.cmd`, whose shebang could resolve to a *different* node and silently
/// install against the wrong version. Candidates cover the Windows layout
/// (node.exe beside `node_modules/`) and the dist layout (`bin/` + `lib/`).
pub(crate) fn find_npm_cli(node_program: &Path) -> Option<PathBuf> {
    let exec = if node_program.as_os_str() == "node" {
        // System node: `node` on PATH. Ask it where it actually lives —
        // once, on a blocking thread via the caller? this fn is sync and the
        // call is cheap next to an npm install.
        let out = std::process::Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if text.is_empty() {
            return None;
        }
        PathBuf::from(text)
    } else {
        node_program.to_path_buf()
    };
    let dir = exec.parent()?;
    let candidates = [
        dir.join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js"),
        dir.parent()?
            .join("lib")
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npm-cli.js"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Pick a Node that can *run npm* at install time: the newest installed
/// runtime that ships its bundled npm-cli, falling back to system node.
/// The choice is irrelevant to what the instance executes — this node only
/// materialises `versions/<name>/node_modules`.
pub(crate) fn pick_npm_capable_node(root: &Path) -> Option<PathBuf> {
    let dir = root.join("runtimes");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort_by(|a, b| b.cmp(a));
    for name in names {
        let base = dir.join(&name);
        let node = if cfg!(windows) {
            base.join(crate::launch::node_binary())
        } else {
            base.join("bin").join(crate::launch::node_binary())
        };
        if node.exists() && find_npm_cli(&node).is_some() {
            return Some(node);
        }
    }
    None
}

/// Strip the version's declared devDependencies before running npm.
///
/// dsh is a monorepo package: its dev deps reference unpublished workspace
/// members (`@deepseek-ai/dsh-experimental-*`), and npm resolves the *full*
/// tree — dev deps included — even with `--omit=dev`, so one phantom package
/// 404s the entire runtime install. Dev deps can never be needed at launch,
/// so they come out of a scratch manifest first; the original bytes are
/// restored before `install_version_deps` returns.
pub(crate) fn strip_dev_dependencies(version_dir: &Path) -> bool {
    let path = version_dir.join("package.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut pkg) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(map) = pkg.as_object_mut() else {
        return false;
    };
    if map
        .get("devDependencies")
        .and_then(|v| v.as_object())
        .map(|o| o.is_empty())
        .unwrap_or(true)
    {
        return false;
    }
    map.remove("devDependencies");
    let Ok(modified) = serde_json::to_string_pretty(&pkg) else {
        return false;
    };
    std::fs::write(&path, modified).is_ok()
}

/// Remove a named dependency from the version's package.json; true if the
/// manifest changed. The caller holds the original bytes and restores them
/// once node_modules exists — the pruning is solver input, never the shipped
/// manifest.
pub(crate) fn remove_dep_from_manifest(version_dir: &Path, name: &str) -> bool {
    let path = version_dir.join("package.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(mut pkg) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(map) = pkg.as_object_mut() else {
        return false;
    };
    let mut changed = false;
    for key in ["dependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(deps) = map.get_mut(key).and_then(|d| d.as_object_mut()) {
            changed |= deps.remove(name).is_some();
        }
    }
    if !changed {
        return false;
    }
    let Ok(modified) = serde_json::to_string_pretty(&pkg) else {
        return false;
    };
    std::fs::write(&path, modified).is_ok()
}

/// The official npm registry. Mirrors can lag or drop packages; a 404 from a
/// mirror is retried here once before any prune decision is made.
pub(crate) const OFFICIAL_NPM_REGISTRY: &str = "https://registry.npmjs.org";

/// The only dependencies a 404 may ever drop from the scratch manifest.
///
/// DSH ships a workspace manifest: its experimental sub-packages are declared
/// as (dev-)dependencies but are not always published, so npm's full-tree
/// solve 404s on them even though nothing imports them at runtime. Anything
/// outside this prefix 404ing is a real problem — silently pruning a core
/// dependency must never be able to look like a successful install.
pub(crate) const PRUNE_ALLOWLIST: &[&str] = &["@deepseek-ai/dsh-experimental-"];

pub(crate) fn prune_allowed(pkg: &str) -> bool {
    PRUNE_ALLOWLIST.iter().any(|prefix| pkg.starts_with(prefix))
}

/// What the retry loop does with a 404 on `pkg`.
pub(crate) enum NotFoundAction {
    /// The current registry is a mirror that may simply lag: retry the whole
    /// solve against the official registry before deciding anything.
    RetryOfficial,
    /// Confirmed missing on the official registry, and allowlisted: drop it
    /// from the scratch manifest and retry. The install is marked degraded.
    Prune,
    /// Confirmed missing and not allowlisted: a required dependency is gone,
    /// and the install must fail rather than ship without it.
    Fatal,
}

pub(crate) fn decide_not_found(
    pkg: &str,
    fallback_pending: bool,
    using_official: bool,
) -> NotFoundAction {
    if !using_official && fallback_pending {
        return NotFoundAction::RetryOfficial;
    }
    if prune_allowed(pkg) {
        NotFoundAction::Prune
    } else {
        NotFoundAction::Fatal
    }
}

/// Pull the package name out of npm's not-found line:
/// `npm error 404  The requested resource '@scope/name@^1.0.0' could not be found`
pub(crate) fn parse_404_package(stderr_text: &str) -> Option<String> {
    for line in stderr_text.lines() {
        const MARKER: &str = "The requested resource '";
        let Some(idx) = line.find(MARKER) else {
            continue;
        };
        let rest = &line[idx + MARKER.len()..];
        let Some(end) = rest.find('\'') else { continue };
        let spec = &rest[..end]; // `@scope/name@range` or `name@range`
        let at = spec.rfind('@')?;
        if at == 0 {
            continue; // unscoped `name` with no range should not happen; skip anyway
        }
        return Some(spec[..at].to_string());
    }
    None
}
/// Run `npm install` for a version's own dependencies. Dev deps are stripped
/// from the manifest first (see `strip_dev_dependencies`), scripts are never
/// run (this is a package manager, not a build system), and the user's
/// configured registry mirror is honoured. A dependency that still 404s is
/// pruned from the scratch manifest and the solve is retried — refusing to
/// start the whole instance over one phantom package is worse than starting
/// it without an unused experimental plugin. Returns the skipped list.
/// Progress is a slow indeterminate ramp per attempt.
pub(crate) async fn install_version_deps(
    node_program: &Path,
    version_dir: &Path,
    registry_base: &str,
    flag: &AtomicBool,
    on_progress: impl Fn(f64) + Send + Sync,
) -> Result<Vec<String>, String> {
    let npm_cli = find_npm_cli(node_program).ok_or(
        "未找到 Node 自带的 npm-cli.js，无法安装 DSH 版本依赖。请更换实例绑定的 Runtime 为 PHL 安装的 Node，或重新安装 Node。",
    )?;
    // The manifest edits below are solver input only; the shipped bytes come
    // back before this function returns, success or not.
    let original_manifest =
        std::fs::read_to_string(version_dir.join("package.json")).unwrap_or_default();
    let mut skipped: Vec<String> = Vec::new();
    strip_dev_dependencies(version_dir);

    let outcome = install_deps_attempts(
        node_program,
        &npm_cli,
        version_dir,
        registry_base,
        flag,
        &on_progress,
        &mut skipped,
    )
    .await;

    if !original_manifest.is_empty() {
        let _ = std::fs::write(version_dir.join("package.json"), &original_manifest);
    }
    let skipped = outcome?;
    // The install succeeded, but a pruned dependency means the environment is
    // a known-degraded derivative of the manifest — recorded where both the
    // verifier and the UI can find it.
    let marker = serde_json::json!({
        "installedAt": now_iso(),
        "installHealth": if skipped.is_empty() { "healthy" } else { "degraded" },
        "skipped": skipped,
    });
    let _ = std::fs::write(version_dir.join("phl-deps.json"), marker.to_string());
    on_progress(1.0);
    Ok(skipped)
}

/// One npm attempt per loop pass; a resolvable 404 prunes and retries.
#[allow(clippy::too_many_arguments)]
async fn install_deps_attempts(
    node_program: &Path,
    npm_cli: &Path,
    version_dir: &Path,
    registry_base: &str,
    flag: &AtomicBool,
    on_progress: &(dyn Fn(f64) + Send + Sync),
    skipped: &mut Vec<String>,
) -> Result<Vec<String>, String> {
    let mut effective_registry = registry_base.to_string();
    // A mirror 404 gets exactly one retry against the official registry
    // before any prune decision — a lagging mirror must not read as "the
    // package is gone".
    let mut fallback_pending = registry_base.trim_end_matches('/') != OFFICIAL_NPM_REGISTRY;

    for attempt in 0..12_u8 {
        match run_npm_install(
            node_program,
            npm_cli,
            version_dir,
            &effective_registry,
            flag,
            on_progress,
        )
        .await
        {
            Ok(()) => return Ok(std::mem::take(skipped)),
            Err(e) if e == "cancelled" => return Err("cancelled".into()),
            Err(stderr) => {
                let Some(pkg) = parse_404_package(&stderr) else {
                    let tail = stderr
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .rev()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" | ");
                    // A failed solve must not leave a tree that reads as
                    // installed (the launch probe only checks node_modules).
                    let _ = tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                    return Err(if tail.is_empty() {
                        format!("npm install 失败: {stderr}")
                    } else {
                        format!("npm install 失败: {tail}")
                    });
                };
                let using_official = effective_registry == OFFICIAL_NPM_REGISTRY;
                match decide_not_found(&pkg, fallback_pending, using_official) {
                    NotFoundAction::RetryOfficial => {
                        eprintln!(
                            "[phl] 依赖 {pkg} 在 {effective_registry} 上 404，改用官方 registry 重试"
                        );
                        effective_registry = OFFICIAL_NPM_REGISTRY.to_string();
                        fallback_pending = false;
                        continue;
                    }
                    NotFoundAction::Prune => {
                        eprintln!(
                            "[phl] 依赖 {pkg} 官方 registry 也 404，属于可跳过的实验性依赖，从清单中移除后重试"
                        );
                        if !remove_dep_from_manifest(version_dir, &pkg) {
                            let _ =
                                tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                            return Err(format!(
                                "依赖 {pkg} 在 registry 上不存在，且无法从清单中移除它"
                            ));
                        }
                        skipped.push(pkg);
                        if attempt == 11 {
                            let _ =
                                tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                            return Err("跳过的依赖过多，安装中止".into());
                        }
                    }
                    NotFoundAction::Fatal => {
                        eprintln!(
                            "[phl] 依赖 {pkg} 官方 registry 也 404，且不属于可跳过的实验性依赖"
                        );
                        let _ = tokio::fs::remove_dir_all(version_dir.join("node_modules")).await;
                        return Err(format!(
                            "依赖 {pkg} 在 registry 上不存在，且不属于可跳过的实验性依赖，安装中止"
                        ));
                    }
                }
            }
        }
    }
    unreachable!("the loop always returns")
}

/// One `npm install` child. The returned Err is the raw stderr text when the
/// solve failed (the retry loop parses 404s out of it) or "cancelled".
///
/// stderr is *drained while waiting*: npm's error output for a 70-dependency
/// tree is far over the ~64 KB pipe buffer, and a piped-stderr child whose
/// pipe nobody reads blocks mid-write — the wait never returns.
async fn run_npm_install(
    node_program: &Path,
    npm_cli: &Path,
    version_dir: &Path,
    registry_base: &str,
    flag: &AtomicBool,
    on_progress: &(dyn Fn(f64) + Send + Sync),
) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut command = tokio::process::Command::new(node_program);
    command
        .arg(npm_cli)
        .args([
            "install",
            "--omit=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--no-package-lock",
        ])
        .arg("--prefix")
        .arg(version_dir)
        .arg("--registry")
        .arg(registry_base.trim_end_matches('/'))
        .current_dir(version_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(crate::launch::CREATE_NO_WINDOW);

    let mut child = command.spawn().map_err(|e| format!("无法运行 npm: {e}"))?;
    let mut stderr_pipe = child.stderr.take().ok_or("无法读取 npm 输出")?;
    let started = Instant::now();
    let mut err_text = String::new();
    let mut buf = [0u8; 8192];
    let mut child_status: Option<std::process::ExitStatus> = None;
    let mut cancelled = false;

    // A 250 ms tick drives both the cancel poll and the progress ramp; the
    // stderr and wait arms do the rest. (A `select!` guard would be evaluated
    // once per arm and never re-checked, so cancel lives in the tick arm.)
    loop {
        tokio::select! {
            chunk = stderr_pipe.read(&mut buf) => {
                match chunk {
                    Ok(0) => {}
                    Ok(n) => {
                        err_text.push_str(&String::from_utf8_lossy(&buf[..n]));
                        // npm logs can grow past the pipe into MBs; the tail
                        // is all the retry loop and the error toast ever need.
                        if err_text.len() > 64 * 1024 {
                            // Walk forward to a char boundary instead of
                            // slicing at the raw offset: `err_text[..cut]`
                            // panics outright when `cut` lands inside a
                            // multi-byte character, which any non-ASCII npm
                            // output (a CN mirror's messages, box drawing)
                            // makes likely once the tail trim kicks in.
                            let mut start = err_text.len() - 32 * 1024;
                            while start < err_text.len() && !err_text.is_char_boundary(start) {
                                start += 1;
                            }
                            err_text.replace_range(..start, "");
                        }
                    }
                    Err(_) => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if cancelled_flag(flag) {
                    if let Some(pid) = child.id() {
                        let _ = crate::launch::kill_tree(pid).await;
                    }
                    cancelled = true;
                    break;
                }
                let elapsed = started.elapsed().as_secs_f64();
                on_progress(0.95_f64.min(elapsed / (elapsed + 60.0) * 1.9));
            }
            status = child.wait() => {
                child_status = status.ok();
                break;
            }
        }
    }

    if cancelled {
        return Err("cancelled".into());
    }

    // The child is gone, so the pipe drains from buffers and hits EOF at
    // once — the last few stderr lines still belong to the failure message.
    while let Ok(n) = stderr_pipe.read(&mut buf).await {
        if n == 0 {
            break;
        }
        err_text.push_str(&String::from_utf8_lossy(&buf[..n]));
    }

    let status = match child_status {
        Some(s) => s,
        None => child
            .wait()
            .await
            .map_err(|e| format!("无法等待 npm 退出: {e}"))?,
    };
    if status.success() {
        Ok(())
    } else if err_text.trim().is_empty() {
        Err(format!(
            "npm install 失败 (code {})",
            status.code().unwrap_or(-1)
        ))
    } else {
        Err(err_text)
    }
}

fn cancelled_flag(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

//! The shared tree-copy machinery: cloning, snapshots and data-root
//! relocation all copy directories with progress and cancellation, and the
//! skip rules differ per caller (documented on `SkipRule`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::CloneProgress;

pub(crate) fn skipped(name: &str) -> bool {
    name == "logs" || name == "snapshots" || name == ".phl-cache" || name.starts_with(".phl-")
}

/// What a copy is allowed to leave behind.
///
/// This is not a detail: the same `copy_tree` serves cloning an instance and
/// relocating the entire data root, and those want opposite things. Migrating
/// with the clone's filter silently dropped every instance's `snapshots/` and
/// `logs/` and then deleted the source — destroying the user's only rollback
/// points while reporting success.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkipRule {
    /// Copy everything. Required whenever the copy replaces the original.
    Nothing,
    /// Drop run state, but only at the tree's own root. A nested `logs/` deep
    /// inside `node_modules` belongs to whatever package created it and is
    /// part of that package, not PHL's per-instance history.
    RunStateAtRoot,
}

impl SkipRule {
    pub(crate) fn skips(self, name: &str) -> bool {
        self == SkipRule::RunStateAtRoot && skipped(name)
    }
    /// Recursion always descends with `Nothing`: the rule only ever applies to
    /// the entries directly under the root it was given.
    pub(crate) fn inside(self) -> Self {
        SkipRule::Nothing
    }
}

/* ------------------------------ link policy ----------------------------- */

/// What the copy engine does when it meets a filesystem link.
///
/// The blanket "refuse every link" rule was safe for a hand-built tree but
/// fatal for a real one: installing a DSH version lays its `node_modules` out
/// as ~1000 directory links (junctions on Windows, symlinks on Unix) pointing
/// into `<root>/versions/<v>/node_modules/`, so every instance that has ever
/// run `install-deps` was uncloneable, unsnapshotable and unexportable.
/// The old restore path checked nothing and silently followed them —
/// inconsistent *and* an isolation hole — so every copy now goes through this
/// one classifier.
///
/// Policies differ only on **managed** links: those whose target
/// canonicalizes inside `<root>/versions/`, i.e. the shared immutable install
/// PHL manages. Every other link — escaping the data root, resolving to
/// another instance's mutable home, or dangling — is refused by *all*
/// policies. Classification goes through `std::fs::canonicalize`, which
/// resolves reparse points (Windows junctions, Unix symlinks) and collapses
/// `..` in one step, so an escaping link cannot masquerade as managed.
#[derive(Debug)]
pub(crate) enum LinkPolicy {
    /// Clone / snapshot / restore / adoption: recreate a managed link in the
    /// destination. Two kinds qualify.
    ///
    /// A link into the shared `<root>/versions/` tree keeps pointing at the
    /// **same** target: that tree is immutable and outlives every copy, so the
    /// new tree needs no materialized 282 MB of it — and a snapshot must stay
    /// cheap, which materializing would destroy.
    ///
    /// A link whose target sits **inside the tree being copied** follows the
    /// copy instead (`dest_root` + the same relative path). pnpm lays a
    /// plugin's dependencies out exactly that way — `node_modules/<dep>` is a
    /// junction into `node_modules/.pnpm/<dep>@<v>/…` — and refusing it made
    /// every instance with a dependency-bearing plugin unclonable, which is
    /// the same defect the version links used to cause, one level down.
    ///
    /// Everything else still refuses: the copy never grows a reference to
    /// mutable state it does not own, so clone isolation holds where it matters.
    Preserve {
        /// Root of the tree being copied (a clone's source instance, a
        /// snapshot's `dsh-home`), canonicalized on use.
        source_root: PathBuf,
        /// Where that tree is landing, for rebuilding in-tree links.
        dest_root: PathBuf,
    },
    /// Data-root relocation: `versions/` moves *with* the root, so a link is
    /// recreated pointing at the same relative location under the new root —
    /// the old absolute prefix would dangle the moment the source is deleted.
    Rewrite {
        new_root: PathBuf,
        match_on: LinkMatch,
        /// The subtree being moved. Only targets under it (or under the shared
        /// `versions/`) are translated: a link into some other part of the old
        /// root would be re-pointed at a directory the migration never moves,
        /// so it stays refused.
        source_root: PathBuf,
    },
}

/// How a link's target is matched against the root it is being moved away from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkMatch {
    /// Resolve the link and require its real target to sit under
    /// `<root>/versions/`. Every copy path uses this: the source tree is
    /// user-influenced, so containment must be decided on the canonical path
    /// and an escaping link cannot masquerade as managed.
    Canonical,
    /// Match the link's raw target text. Undoing a migration needs it: the
    /// links the forward pass wrote point at a `versions/` that may not have
    /// moved yet — and therefore does not exist — which canonical resolution
    /// cannot classify at all.
    Raw,
}

/// What the engine does with one source-tree link under the active policy.
pub(crate) enum LinkAction {
    /// Recreate the link at the destination's matching position.
    Recreate {
        /// The target as the policy resolved it (canonical or raw, per
        /// `LinkMatch`) — what the link pointed at before the rewrite.
        previous: PathBuf,
        /// Target for the destination, already rewritten when the policy calls
        /// for it.
        target: PathBuf,
        dir: bool,
    },
    /// Refuse the whole copy, with a user-facing reason.
    Refuse(String),
}

impl LinkPolicy {
    /// Recursion keeps the policy: a managed link deep in a copied tree is
    /// the same managed link at any depth. Borrowed rather than cloned — a
    /// `Rewrite` owns the destination root and is consulted once per link.
    pub(crate) fn inside(&self) -> &Self {
        self
    }

    /// Classify and resolve one link. `data_root` is the root the policy is
    /// evaluated against (the source root for `Preserve`, the root being left
    /// behind for `Rewrite`). Containment is decided on the *canonical* target
    /// for every copy path — `std::fs::canonicalize` resolves every reparse
    /// point and collapses `..`, so a link cannot hop out of the boundary one
    /// segment at a time — and on the raw target text only when a migration is
    /// being undone (see `LinkMatch`).
    pub(crate) fn action(&self, link: &Path, data_root: &Path) -> LinkAction {
        let match_on = match self {
            LinkPolicy::Rewrite { match_on, .. } => *match_on,
            LinkPolicy::Preserve { .. } => LinkMatch::Canonical,
        };
        let previous = match match_on {
            LinkMatch::Canonical => match std::fs::canonicalize(link) {
                Ok(t) => crate::paths::strip_verbatim(&t),
                Err(_) => {
                    return LinkAction::Refuse(format!(
                        "实例目录包含断开的链接（目标不存在），请先处理: {}",
                        link.display()
                    ))
                }
            },
            LinkMatch::Raw => match std::fs::read_link(link) {
                Ok(t) => crate::paths::strip_verbatim(&t),
                Err(_) => {
                    return LinkAction::Refuse(format!(
                        "实例目录包含无法读取的链接，请先处理: {}",
                        link.display()
                    ))
                }
            },
        };
        let root = match match_on {
            LinkMatch::Canonical => match std::fs::canonicalize(data_root) {
                Ok(r) => crate::paths::strip_verbatim(&r),
                Err(_) => {
                    return LinkAction::Refuse("数据根目录不可读，无法判定链接归属".to_string())
                }
            },
            LinkMatch::Raw => crate::paths::strip_verbatim(data_root),
        };
        // The reparse point itself says whether this is a directory link — a
        // dangling junction cannot be resolved to find out.
        let dir = link_is_dir(link);
        match self {
            LinkPolicy::Preserve {
                source_root,
                dest_root,
            } => {
                // Shared, immutable version install: keep pointing at it.
                if previous.starts_with(root.join("versions")) {
                    return LinkAction::Recreate {
                        target: previous.clone(),
                        previous,
                        dir,
                    };
                }
                // A link inside the tree being copied travels with the copy.
                let source = match std::fs::canonicalize(source_root) {
                    Ok(p) => crate::paths::strip_verbatim(&p),
                    Err(_) => {
                        return LinkAction::Refuse("复制源目录不可读，无法判定链接归属".to_string())
                    }
                };
                match previous.strip_prefix(&source) {
                    Ok(rel) => LinkAction::Recreate {
                        target: crate::paths::strip_verbatim(dest_root).join(rel),
                        previous,
                        dir,
                    },
                    // Inside the root but outside the copied tree (e.g. a
                    // sibling instance's home), or outside entirely: a copy
                    // must not inherit a reference to state it does not own.
                    Err(_) => LinkAction::Refuse(format!(
                        "实例目录包含指向受管版本之外位置的链接，请先处理: {}",
                        link.display()
                    )),
                }
            }
            LinkPolicy::Rewrite {
                new_root,
                source_root,
                ..
            } => {
                let moving = match match_on {
                    LinkMatch::Canonical => match std::fs::canonicalize(source_root) {
                        Ok(p) => crate::paths::strip_verbatim(&p),
                        Err(_) => {
                            return LinkAction::Refuse(
                                "迁移源目录不可读，无法判定链接归属".to_string(),
                            )
                        }
                    },
                    LinkMatch::Raw => crate::paths::strip_verbatim(source_root),
                };
                // A raw target is text, so a `..` inside it could pass the
                // prefix test and then resolve outside the destination.
                if previous
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return LinkAction::Refuse(format!(
                        "实例目录包含带有相对路径段的链接，请先处理: {}",
                        link.display()
                    ));
                }
                let translatable =
                    previous.starts_with(root.join("versions")) || previous.starts_with(&moving);
                let Ok(rel) = previous.strip_prefix(&root) else {
                    return LinkAction::Refuse(format!(
                        "实例目录包含指向受管版本之外位置的链接，请先处理: {}",
                        link.display()
                    ));
                };
                if !translatable {
                    return LinkAction::Refuse(format!(
                        "实例目录包含指向受管版本之外位置的链接，请先处理: {}",
                        link.display()
                    ));
                }
                // The whole root moves, so every target under it is translated
                // to the same relative place. `versions/` may not exist yet
                // mid-migration, so this is a pure path translation.
                let target = crate::paths::strip_verbatim(new_root).join(rel);
                LinkAction::Recreate {
                    previous,
                    target,
                    dir,
                }
            }
        }
    }
}

/// Whether a link is a directory link, decided from the reparse point itself:
/// a dangling junction cannot be resolved to find out, and `mklink /J` — the
/// kind npm lays out — carries the directory attribute.
fn link_is_dir(link: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;
        std::fs::symlink_metadata(link)
            .map(|m| m.file_type().is_symlink_dir())
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        let _ = link;
        false
    }
}

/// Recreate a link at `dst` pointing at `target`, in whatever kind the
/// platform needs and the target's shape allows.
///
/// Windows is the careful one: PHL has no privilege to create real symlinks
/// (that needs dev-mode or admin), but a **junction** — the exact kind npm and
/// pnpm lay out `versions/node_modules` with — needs none, so directory links
/// go through `mklink /J` and only file links attempt the symlink API.
pub(crate) fn recreate_link(dst: &Path, target: &Path, as_dir: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        if as_dir {
            let junction = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(dst)
                .arg(target)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if junction {
                return Ok(());
            }
            std::os::windows::fs::symlink_dir(target, dst)
                .map_err(|e| format!("无法重建链接 {}: {e}", dst.display()))
        } else {
            std::os::windows::fs::symlink_file(target, dst)
                .map_err(|e| format!("无法重建链接 {}: {e}", dst.display()))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = as_dir;
        std::os::unix::fs::symlink(target, dst)
            .map_err(|e| format!("无法重建链接 {}: {e}", dst.display()))
    }
}

/// Rewrite the managed links inside an already-moved tree so they point at the
/// other root.
///
/// The same-drive relocation fast-path renames a directory wholesale, which
/// preserves the links but leaves each junction's absolute target pointing at
/// the root being left behind — a tree that resolves until that root is
/// deleted, then silently dangles. Walking the moved subtree and re-pointing
/// every managed link restores the guarantee the copy path builds in from the
/// start. Classification is `LinkPolicy::Rewrite` — the same code the copy
/// path runs — so the fast path and the copy path cannot disagree about what
/// "managed" means.
///
/// **Two phases, deliberately.** Nothing is mutated until every link in the
/// tree has been classified, because the caller's rollback only moves the
/// directory back: a link already re-pointed would survive as a reference into
/// a root that does not exist yet (`versions/` is migrated *after*
/// `instances/`). A refusal therefore leaves the tree exactly as it was.
///
/// `from_root` is the root the links currently resolve against; `to_root` is
/// where they must point afterwards. `match_on` decides how the current target
/// is read — see `LinkMatch`: the forward migration canonicalizes, undoing one
/// must read the raw text because the new `versions/` may not exist yet.
pub(crate) fn repoint_managed_links(
    dir: &Path,
    from_root: &Path,
    to_root: &Path,
    match_on: LinkMatch,
) -> Result<(), String> {
    // The caller hands us `<to_root>/<kind>`; the links inside still point at
    // `<from_root>/<kind>`, which is exactly the subtree the move relocates.
    let kind = dir.strip_prefix(to_root).unwrap_or(Path::new(""));
    let policy = LinkPolicy::Rewrite {
        new_root: to_root.to_path_buf(),
        match_on,
        source_root: from_root.join(kind),
    };
    // Phase 1: classify only.
    let mut planned: Vec<(PathBuf, PathBuf, PathBuf, bool)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current)
            .map_err(|e| format!("迁移后遍历 {} 失败: {e}", current.display()))?
            .flatten()
        {
            let path = entry.path();
            // DirEntry::file_type does not follow reparse points.
            if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
                match policy.action(&path, from_root) {
                    // The policy's wording is framed for a copy; a migration
                    // reads better with its own.
                    LinkAction::Refuse(why) => return Err(why.replace("实例目录", "迁移目标")),
                    LinkAction::Recreate {
                        previous,
                        target,
                        dir,
                    } => planned.push((path, previous, target, dir)),
                }
            } else if entry.metadata().map(|m| m.is_dir()).unwrap_or(false) {
                stack.push(path);
            }
        }
    }
    apply_repoint(planned)
}

/// Apply a fully-classified rewrite plan.
///
/// A failure here is I/O, not policy, so the links already rewritten are put
/// back before the error is returned: the caller's rollback can undo a
/// directory move but never a link.
fn apply_repoint(planned: Vec<(PathBuf, PathBuf, PathBuf, bool)>) -> Result<(), String> {
    let mut applied: Vec<usize> = Vec::new();
    for (index, (path, _previous, target, as_dir)) in planned.iter().enumerate() {
        if let Err(e) =
            remove_link(path, *as_dir).and_then(|()| recreate_link(path, target, *as_dir))
        {
            let mut restore = applied.clone();
            restore.push(index);
            let mut failed = Vec::new();
            for i in restore.iter().rev() {
                let (p, old, _, is_dir) = &planned[*i];
                let _ = remove_link(p, *is_dir);
                if recreate_link(p, old, *is_dir).is_err() {
                    failed.push(p.display().to_string());
                }
            }
            return Err(if failed.is_empty() {
                format!("{e}（已还原此前改写的链接）")
            } else {
                format!("{e}；另有链接未能还原: {}", failed.join(", "))
            });
        }
        applied.push(index);
    }
    Ok(())
}

/// Delete a link without following it (`remove_dir` on a junction would
/// otherwise be a directory removal on the target's children).
fn remove_link(path: &Path, as_dir: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        // A junction is a reparse point: remove_dir deletes the reparse, not
        // the shared target tree. Files go through remove_file.
        let r = if as_dir {
            std::fs::remove_dir(path)
        } else {
            std::fs::remove_file(path)
        };
        r.map_err(|e| format!("无法移除旧链接 {}: {e}", path.display()))
    }
    #[cfg(not(windows))]
    {
        let _ = as_dir;
        std::fs::remove_file(path).map_err(|e| format!("无法移除旧链接 {}: {e}", path.display()))
    }
}

/// Bytes of *ordinary* content under `dir`, the same measure `copy_tree`
/// writes out. Links are counted at zero (they are recreated, their targets
/// are shared content measured elsewhere) and never followed — following a
/// managed link would make every instance report the full version tree's
/// size, and `storage.rs` verifies a migration by comparing source and staged
/// `dir_size`, which only balances if both sides ignore links identically.
pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        // `file_type` from a DirEntry does not follow reparse points, so a
        // junction/symlink is seen as a link, not its target's shape.
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

pub(crate) fn dir_size_skipping(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0u64;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if skipped(&name) {
            continue;
        }
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

/// Shared by cloning, snapshots and cross-drive relocation. Completion comes
/// from the worker result, never from a closed progress channel. Awaiting the
/// worker also ensures cancellation cannot race staging-directory cleanup.
pub(crate) async fn copy_tree_with_progress<F: Fn(CloneProgress) + Send + Sync>(
    from: PathBuf,
    to: PathBuf,
    flag: Arc<AtomicBool>,
    skip: SkipRule,
    links: LinkPolicy,
    data_root: PathBuf,
    on_progress: &F,
) -> Result<u64, String> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<(u64, u64), String>>(16);
    let worker = tokio::task::spawn_blocking(move || {
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let total = match skip {
            SkipRule::Nothing => dir_size(&from),
            SkipRule::RunStateAtRoot => dir_size_skipping(&from),
        };
        let mut done = 0;
        let ctx = CopyCtx {
            flag: &flag,
            total,
            tx: &tx,
            skip,
            links: &links,
            data_root: &data_root,
        };
        copy_tree(&from, &to, &mut done, &ctx)?;
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        Ok(done)
    });
    while let Some(message) = rx.recv().await {
        let (bytes_done, bytes_total) = message?;
        on_progress(CloneProgress {
            progress: if bytes_total == 0 {
                1.0
            } else {
                (bytes_done as f64 / bytes_total as f64).min(1.0)
            },
            bytes_done,
            bytes_total,
        });
    }
    worker.await.map_err(|e| format!("复制线程异常退出: {e}"))?
}

/// What one copy needs that does not change as it recurses: the cancellation
/// flag, the running and total byte counters, the progress sink, and the two
/// policies. Grouped so the recursive signature stays readable.
pub(crate) struct CopyCtx<'a> {
    pub(crate) flag: &'a AtomicBool,
    pub(crate) total: u64,
    pub(crate) tx: &'a tokio::sync::mpsc::Sender<Result<(u64, u64), String>>,
    pub(crate) skip: SkipRule,
    pub(crate) links: &'a LinkPolicy,
    pub(crate) data_root: &'a Path,
}

pub(crate) fn copy_tree(
    from: &Path,
    to: &Path,
    done: &mut u64,
    ctx: &CopyCtx<'_>,
) -> Result<(), String> {
    let CopyCtx {
        flag,
        total,
        tx,
        skip,
        links,
        data_root,
    } = *ctx;
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    let entries = std::fs::read_dir(from).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读取复制源目录失败: {e}"))?;
        if flag.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let name = entry.file_name();
        if skip.skips(&name.to_string_lossy()) {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        let meta = entry
            .metadata()
            .map_err(|e| format!("读取复制源属性失败 {}: {e}", src.display()))?;
        // A relocation deletes the source afterwards, so silently skipping a
        // link (or following it outside the tree) would lose or duplicate
        // data. What actually happens is the caller's policy: managed links
        // (inside `<root>/versions/`) are recreated, everything else is
        // refused. Links are never counted — `dir_size` measures them at zero
        // — so the progress totals stay balanced.
        if entry.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            match links.action(&src, data_root) {
                LinkAction::Refuse(reason) => return Err(reason),
                LinkAction::Recreate { target, dir, .. } => {
                    recreate_link(&dst, &target, dir)?;
                    continue;
                }
            }
        }
        if meta.is_dir() {
            let inner = CopyCtx {
                skip: skip.inside(),
                links: links.inside(),
                ..*ctx
            };
            copy_tree(&src, &dst, done, &inner)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("复制失败 {}: {e}", src.display()))?;
            *done += meta.len();
            // A closed channel means the caller gave up; stop rather than keep
            // writing into a directory it is already deleting.
            if tx.blocking_send(Ok((*done, total))).is_err() {
                return Err("cancelled".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-copy-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A directory link of the kind npm lays out for `versions/node_modules`:
    /// a junction on Windows (needs no privilege), a symlink elsewhere.
    fn dir_link(target: &Path, link: &Path) {
        recreate_link(link, target, true).expect("create dir link");
    }

    /// A populated fake data root: `<root>/versions/v1/node_modules/dep`.
    fn seed_versions(root: &Path) -> PathBuf {
        let versions = root.join("versions").join("v1").join("node_modules");
        std::fs::create_dir_all(versions.join("dep")).unwrap();
        std::fs::write(versions.join("dep").join("index.js"), b"module").unwrap();
        versions
    }

    fn is_link(path: &Path) -> bool {
        std::fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }

    #[test]
    fn a_managed_link_is_recreated_and_a_non_managed_link_is_refused() {
        let root = tmp("link-policy");
        let versions = seed_versions(&root);
        let instance = root.join("instances").join("a");
        std::fs::create_dir_all(&instance).unwrap();
        let managed = instance.join("node_modules");
        dir_link(&versions, &managed);

        // Canonicalize the root: on macOS the temp dir is a symlink
        // (`/var` → `/private/var`), and the classifier compares canonical
        // paths on both sides.
        let real_root = std::fs::canonicalize(&root).unwrap();

        let copy_policy = || LinkPolicy::Preserve {
            source_root: instance.clone(),
            dest_root: root.join("copy-of-a"),
        };
        match copy_policy().action(&managed, &real_root) {
            LinkAction::Recreate { dir, target, .. } => {
                assert!(dir, "a directory link is rebuilt as a directory link");
                assert_eq!(
                    target,
                    crate::paths::strip_verbatim(&std::fs::canonicalize(&versions).unwrap())
                );
            }
            LinkAction::Refuse(why) => panic!("managed link refused: {why}"),
        }

        // A link into a sibling instance's mutable home is refused by every
        // policy: a copy must never inherit state it does not own.
        let sibling = root.join("instances").join("b");
        std::fs::create_dir_all(&sibling).unwrap();
        let borrowed = instance.join("borrowed");
        dir_link(&sibling, &borrowed);
        match copy_policy().action(&borrowed, &real_root) {
            LinkAction::Refuse(_) => {}
            LinkAction::Recreate { .. } => panic!("non-managed link was accepted"),
        }

        // A dangling link is refused too — its target cannot be classified.
        let dangling = instance.join("dangling");
        dir_link(&root.join("versions").join("gone"), &dangling);
        match copy_policy().action(&dangling, &real_root) {
            LinkAction::Refuse(why) => assert!(why.contains("断开"), "got: {why}"),
            LinkAction::Recreate { .. } => panic!("dangling link was accepted"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rewrite_repoints_a_managed_link_under_the_new_root() {
        let root = tmp("link-rewrite");
        let versions = seed_versions(&root);
        let instance = root.join("instances").join("a");
        std::fs::create_dir_all(&instance).unwrap();
        let managed = instance.join("node_modules");
        dir_link(&versions, &managed);

        let new_root = root.join("new-root");
        let real_root = std::fs::canonicalize(&root).unwrap();
        let policy = LinkPolicy::Rewrite {
            new_root: new_root.clone(),
            match_on: LinkMatch::Canonical,
            source_root: instance.clone(),
        };
        match policy.action(&managed, &real_root) {
            LinkAction::Recreate { target, .. } => {
                // The old absolute prefix would dangle once the source root is
                // deleted; the link is translated to the same relative place.
                assert_eq!(
                    target,
                    new_root.join("versions").join("v1").join("node_modules")
                );
            }
            LinkAction::Refuse(why) => panic!("managed link refused: {why}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_tree_with_junctions_copies_instead_of_failing() {
        // Defect #4 regression at the engine level: before `LinkPolicy` was
        // wired in, `copy_tree` returned an error on the first link, so any
        // instance that had ever run install-deps could not be cloned,
        // snapshotted or exported.
        let root = tmp("link-copy");
        let versions = seed_versions(&root);
        let from = root.join("instances").join("src");
        std::fs::create_dir_all(from.join("dsh-home")).unwrap();
        std::fs::write(from.join("dsh-home").join("settings.yaml"), b"abcde").unwrap();
        let link = from.join("profiles").join("node_modules");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        dir_link(&versions, &link);

        let to = root.join("instances").join("dst");
        let copied = copy_tree_with_progress(
            from.clone(),
            to.clone(),
            Arc::new(AtomicBool::new(false)),
            SkipRule::Nothing,
            LinkPolicy::Preserve {
                source_root: from.clone(),
                dest_root: to.clone(),
            },
            std::fs::canonicalize(&root).unwrap(),
            &|_| {},
        )
        .await
        .expect("a tree with managed links must copy");

        assert_eq!(
            copied, 5,
            "links contribute no bytes — the target is shared"
        );
        let copied_link = to.join("profiles").join("node_modules");
        assert!(
            is_link(&copied_link),
            "the link is recreated, not materialized"
        );
        assert!(
            copied_link.join("dep").join("index.js").exists(),
            "the recreated link still resolves to the shared tree"
        );
        assert!(to.join("dsh-home").join("settings.yaml").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_escaping_link_still_refuses_the_whole_copy() {
        let root = tmp("link-copy-refuse");
        let _versions = seed_versions(&root);
        let from = root.join("instances").join("src");
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("keep.txt"), b"x").unwrap();
        let sibling = root.join("instances").join("other");
        std::fs::create_dir_all(&sibling).unwrap();
        dir_link(&sibling, &from.join("borrowed"));

        let dest = root.join("instances").join("dst");
        let err = copy_tree_with_progress(
            from.clone(),
            dest.clone(),
            Arc::new(AtomicBool::new(false)),
            SkipRule::Nothing,
            LinkPolicy::Preserve {
                source_root: from.clone(),
                dest_root: dest,
            },
            std::fs::canonicalize(&root).unwrap(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("受管版本之外"), "got: {err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refusing_a_link_leaves_every_earlier_link_untouched() {
        // The two-phase guarantee. The caller's rollback moves the directory
        // back, which cannot undo a link: a half-rewritten tree would survive
        // as a reference into a root that does not exist yet (`versions/`
        // migrates *after* `instances/`).
        let root = tmp("repoint-atomic");
        let from_root = root.join("old");
        let to_root = root.join("new");
        let dep = from_root
            .join("versions")
            .join("v1")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&dep).unwrap();
        std::fs::write(dep.join("index.js"), b"shared").unwrap();

        // The tree is already at its destination (the rename happened first),
        // and its links still name the old root — exactly what the fast path
        // hands `repoint_managed_links`.
        let tree = to_root.join("instances").join("a");
        std::fs::create_dir_all(&tree).unwrap();
        // Names decide classification order (NTFS returns entries in name
        // order): the managed link is reached first, the escaping one second.
        let managed = tree.join("a-deps");
        dir_link(
            &from_root.join("versions").join("v1").join("node_modules"),
            &managed,
        );
        let elsewhere = from_root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        dir_link(&elsewhere, &tree.join("z-borrowed"));

        let before = crate::paths::strip_verbatim(&std::fs::read_link(&managed).unwrap());
        let err = repoint_managed_links(&tree, &from_root, &to_root, LinkMatch::Raw).unwrap_err();
        assert!(err.contains("受管版本之外"), "got: {err}");
        let after = crate::paths::strip_verbatim(&std::fs::read_link(&managed).unwrap());
        assert_eq!(
            before, after,
            "a refusal must not rewrite any link, even one already classified"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn raw_matching_translates_a_link_whose_target_does_not_exist_yet() {
        // Undoing a migration: the forward pass wrote links into `to/versions`,
        // which may never have arrived. Canonical matching cannot even read
        // them (the target does not resolve); raw matching translates them.
        let root = tmp("repoint-raw");
        let from_root = root.join("old");
        let to_root = root.join("new");
        std::fs::create_dir_all(from_root.join("versions").join("v1").join("node_modules"))
            .unwrap();
        let tree = from_root.join("instances").join("a");
        std::fs::create_dir_all(&tree).unwrap();
        let link = tree.join("node_modules");
        dir_link(
            &to_root.join("versions").join("v1").join("node_modules"),
            &link,
        );

        let canonical = LinkPolicy::Rewrite {
            new_root: from_root.clone(),
            match_on: LinkMatch::Canonical,
            source_root: to_root.clone(),
        };
        assert!(
            matches!(canonical.action(&link, &to_root), LinkAction::Refuse(_)),
            "a dangling target cannot be classified canonically"
        );

        repoint_managed_links(&tree, &to_root, &from_root, LinkMatch::Raw).unwrap();
        assert_eq!(
            crate::paths::strip_verbatim(&std::fs::read_link(&link).unwrap()),
            from_root.join("versions").join("v1").join("node_modules"),
            "the link points back at the source root"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_link_inside_the_copied_tree_follows_the_copy() {
        // pnpm's isolated layout, measured on Windows: `node_modules/<dep>` is
        // a junction into `node_modules/.pnpm/<dep>@<v>/node_modules/<dep>`, i.e.
        // *inside* the instance. Refusing it made every instance with a
        // dependency-bearing plugin unclonable — the same defect the version
        // links used to cause, one level down.
        let root = tmp("link-in-tree");
        let from = root.join("instances").join("src");
        let store = from
            .join("node_modules")
            .join(".pnpm")
            .join("dep@1.0.0")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::write(store.join("index.js"), b"pnpm-dep").unwrap();
        let link = from.join("node_modules").join("dep");
        dir_link(&store, &link);

        let to = root.join("instances").join("dst");
        let copied = copy_tree_with_progress(
            from.clone(),
            to.clone(),
            Arc::new(AtomicBool::new(false)),
            SkipRule::Nothing,
            LinkPolicy::Preserve {
                source_root: from.clone(),
                dest_root: to.clone(),
            },
            std::fs::canonicalize(&root).unwrap(),
            &|_| {},
        )
        .await
        .expect("a pnpm-style tree must copy");

        let copied_link = to.join("node_modules").join("dep");
        assert!(is_link(&copied_link), "the junction survives as a link");
        let target = std::fs::canonicalize(&copied_link).unwrap();
        assert!(
            target.starts_with(std::fs::canonicalize(&to).unwrap()),
            "the rebuilt link must point inside the copy, not back at the source: {}",
            target.display()
        );
        assert_eq!(
            std::fs::read(copied_link.join("index.js")).unwrap(),
            b"pnpm-dep"
        );
        assert_eq!(copied, 8, "only the dependency's own bytes are counted");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rewrite_translates_an_in_tree_link_with_the_root() {
        // The migration half: the whole root moves, so a link into the
        // instance's own `.pnpm` must be translated to the new root,
        // exactly like a link into `versions/`.
        let root = tmp("link-rewrite-in-tree");
        let old_root = root.join("old");
        let new_root = root.join("new");
        let store = old_root
            .join("instances")
            .join("a")
            .join("node_modules")
            .join(".pnpm")
            .join("dep@1.0.0")
            .join("node_modules")
            .join("dep");
        std::fs::create_dir_all(&store).unwrap();
        let link = old_root
            .join("instances")
            .join("a")
            .join("node_modules")
            .join("dep");
        dir_link(&store, &link);

        let policy = LinkPolicy::Rewrite {
            new_root: new_root.clone(),
            match_on: LinkMatch::Canonical,
            source_root: old_root.join("instances"),
        };
        match policy.action(&link, &std::fs::canonicalize(&old_root).unwrap()) {
            LinkAction::Recreate { target, .. } => assert_eq!(
                target,
                new_root
                    .join("instances")
                    .join("a")
                    .join("node_modules")
                    .join(".pnpm")
                    .join("dep@1.0.0")
                    .join("node_modules")
                    .join("dep")
            ),
            LinkAction::Refuse(why) => panic!("in-tree link refused: {why}"),
        }

        let _ = std::fs::remove_dir_all(&root);
    }
}

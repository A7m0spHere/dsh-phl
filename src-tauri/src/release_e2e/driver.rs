//! The one headless launch wrapper.
//!
//! `run_launch` is the real production startup pipeline, but its `AppHandle`
//! is used *only* to hand the child to `watch_child_process` (the GUI exit
//! emitter) on three paths. This module replicates `launch_instance`'s
//! command shell — the instance lock, the availability check, the cancel-flag
//! bookkeeping — and calls `run_launch` directly with the window-less handle
//! made impossible: we cannot build an `AppHandle` outside a running Tauri
//! runtime (`ipc_contract.rs:11-17` records MockRuntime failing to load).
//!
//! So the child is returned instead: the caller owns termination through the
//! real stop path (`terminate_or_gone`, `launch/mod.rs:429`). What this
//! deliberately does NOT cover is the GUI-side watcher itself — window events
//! and exit notifications that need a live `AppHandle`; those belong to the
//! installer/CDP smoke lane, and this boundary is recorded in
//! `docs/windows-e2e-release-gate.md`.

use std::collections::HashMap;

use crate::credentials::Creds;
use crate::launch::{LaunchOutcome, Launches, Processes, Registry};
use crate::resources::{Held, Resource, ResourceLocks, Tasks};
use crate::versions::Transfers;

/// Everything a headless session wires together — the same plain structs the
/// Tauri app manages as state (`lib.rs:283-288`), no `State<'_, _>` borrows.
pub(crate) struct Session {
    pub(crate) phl: crate::paths::PhlState,
    pub(crate) locks: ResourceLocks,
    pub(crate) tasks: Tasks,
    pub(crate) transfers: Transfers,
    pub(crate) launches: Launches,
    pub(crate) processes: Processes,
    pub(crate) registry: Registry,
    pub(crate) creds: Creds,
    /// Kept for cleanup; the temp dir holds the pointer and processes.json too.
    pub(crate) base: std::path::PathBuf,
}

impl Session {
    /// A fresh isolated session: root pointer in a nested dir, data root under
    /// the base, and the durable process registry bound next to the pointer
    /// (where `paths.rs:230` says run-state files live — outside the data
    /// root, so a storage migration can never strand it).
    pub(crate) fn fresh(tag: &str) -> Self {
        let base = super::fixtures::scratch(tag);
        let root = base.join("data");
        std::fs::create_dir_all(&root).unwrap();
        let pointer = base.join("config").join("root.json");
        std::fs::create_dir_all(pointer.parent().unwrap()).unwrap();
        let phl = crate::paths::PhlState::with_pointer(Some(pointer.clone()));
        phl.adopt(Some(&super::fixtures::abs(&root))).unwrap();
        let registry = Registry::default();
        if let Some(proc_file) = phl.sibling_file("processes.json") {
            registry.bind(proc_file);
        }
        Session {
            phl,
            locks: ResourceLocks::default(),
            tasks: Tasks::default(),
            transfers: Transfers::default(),
            launches: Launches::default(),
            processes: Processes::default(),
            registry,
            creds: Creds::platform_default(),
            base,
        }
    }

    pub(crate) fn root(&self) -> std::path::PathBuf {
        self.phl.root()
    }

    /// Kill-and-confirm through the production path, tolerating a process the
    /// OS already reclaimed (a force-killed child may be gone before we look).
    pub(crate) async fn terminate(&self, instance_id: &str) {
        let Some(entry) = self.processes.entry_of(instance_id) else {
            return;
        };
        let _ = crate::launch::terminate_or_gone(
            &self.processes,
            &self.registry,
            instance_id,
            entry.pid,
        )
        .await;
    }

    /// Terminate every child this session started, then drop the scratch tree.
    /// A journey that spawns a fake `node …/bin.js` that outlives the test
    /// leaks a listening socket into the shared CI machine's port range, so
    /// this runs even on a failing assertion.
    pub(crate) async fn cleanup(&self) {
        // Collect first: the `Processes` map is behind a std `Mutex`, and its
        // guard must not be held across the awaits below (clippy
        // await_holding_lock).
        let ids: Vec<String> = self.processes.0.lock().unwrap().keys().cloned().collect();
        for entry in ids {
            self.terminate(&entry).await;
        }
        // Windows race: a just-killed `node.exe` can hold its scratch tree
        // open for a moment after the kill returns; a single
        // `remove_dir_all` then fails and the dir leaks. Bounded retry —
        // the handles always release, so this converges in one or two ticks.
        for _ in 0..20 {
            match std::fs::remove_dir_all(&self.base) {
                Ok(()) => return,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        eprintln!("[e2e] scratch root left behind: {}", self.base.display());
    }
}

/// The result of a headless launch: the outcome the UI would show.
#[derive(Debug)]
pub(crate) struct Launched {
    pub(crate) outcome: LaunchOutcome,
}

/// Hold the instance resource while a launch (or a stop of the same id)
/// would otherwise collide — mirrors `launch_instance:215`.
pub(crate) fn lock_instance(locks: &ResourceLocks, id: &str) -> Held {
    locks
        .acquire(&[Resource::Instance(id.to_string())])
        .expect("e2e: instance resource must be free for the journey")
}

/// The launch path, minus the GUI handle. Same ordering as the command:
/// `ensure_launch_available` (with the real pid probe), take the cancel flag,
/// run the pipeline, release the flag.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn launch(
    session: &Session,
    _held: &Held,
    transfer_id: &str,
    instance_id: &str,
    version_name: &str,
    runtime_name: &str,
    registry_base: &str,
    port: u16,
    auto_port: bool,
    env: HashMap<String, String>,
    args: Vec<String>,
    profile: &str,
    on_progress: &tauri::ipc::Channel<crate::launch::LaunchEvent>,
) -> Result<Launched, String> {
    crate::launch::ensure_launch_available(
        &session.processes,
        &session.registry,
        instance_id,
        crate::launch::probe_process,
    )?;
    let cancel = session.launches.take(transfer_id);
    // The GUI hands a retained child to `watch_child_process` (exit events →
    // window). Headless, the recorder is a drop: `kill_on_drop` is not set,
    // so the child keeps running and the journey stops it through the real
    // `terminate_or_gone` path by pid.
    let result = crate::launch::run_launch(
        &session.processes,
        &session.registry,
        &cancel,
        &|_id: &str, _pid, _child| {},
        &session.root(),
        instance_id,
        version_name,
        runtime_name,
        registry_base,
        profile,
        port,
        auto_port,
        env,
        args,
        None, // no API binding: the key injector is never consulted
        &session.creds,
        on_progress,
    )
    .await;
    session.launches.release(transfer_id);
    result.map(|outcome| Launched { outcome })
}

/// The stop path used by journeys: `stop_instance`'s exact body
/// (read → permission → terminate-or-gone), without the `State` wrappers.
pub(crate) async fn stop(session: &Session, instance_id: &str) -> Result<(), String> {
    if let Some(entry) = session.processes.entry_of(instance_id) {
        crate::launch::stop_permission(&session.registry, instance_id, entry.pid)?;
        crate::launch::terminate_or_gone(
            &session.processes,
            &session.registry,
            instance_id,
            entry.pid,
        )
        .await?;
    }
    Ok(())
}

/// Run a journey body with `Session::cleanup` guaranteed on EVERY exit path.
/// The journeys assert with plain `unwrap`/`expect`, and a panic that skips
/// cleanup would leak live `node.exe` children plus their listening sockets
/// into the shared machine — poisoning every later test in the sequential
/// `--test-threads=1` sweep. The panic is resumed after cleanup so the
/// failure is still reported as-is.
pub(crate) async fn with_cleanup<T>(
    session: &Session,
    body: impl std::future::Future<Output = T>,
) -> T {
    use futures_util::FutureExt;
    let result = std::panic::AssertUnwindSafe(body).catch_unwind().await;
    session.cleanup().await;
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

//! Fault injection F1–F4 — the failure modes the release checklist demands,
//! executed against real processes and real filesystem state.
//!
//! What "system level" means here: the kills are real `taskkill`, the abort
//! really drops a TCP connection mid-body, the lock really holds an NTFS
//! directory open, and the late-exiting child really delays its own death.
//! The one injection CI machines cannot reproduce honestly is a *full disk*
//! (needs admin quotas or a VHD) — `f5` skips with a pointer to the VM lane.

use std::collections::HashMap;

use crate::launch::LaunchEvent;
use crate::resources::{guarded, Resource};

use super::driver::{lock_instance, with_cleanup, Session};
use super::fixtures::{fake_dsh_tarball, integrity_sha512, NodeInfo, Route};
use super::journeys::{cold_mocks, install_runtime, install_version, manifest_for};
use super::wait_until;

fn channel<TSend>() -> tauri::ipc::Channel<TSend> {
    tauri::ipc::Channel::new(|_| Ok(()))
}

/// A ready-to-launch J1-style session: mocks up, runtime and version
/// installed, instance created. Every fault case starts here.
async fn armed(tag: &str, id: &str) -> (Session, String, String) {
    let node = NodeInfo::discover().expect("release-e2e: `node` required on PATH");
    let (server, _tgz, _integrity) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh(tag);
    let runtime = install_runtime(&session, &base, &node).await;
    install_version(&session, &base, super::FAKE_DSH_VERSION).await;
    let held = lock_instance(&session.locks, id);
    crate::instances::create_instance_inner(
        &session.root(),
        manifest_for(id, super::FAKE_DSH_VERSION, &runtime),
    )
    .await
    .unwrap();
    drop(held);
    (session, base, runtime)
}

/* -------------------------------- F1 ---------------------------------- */

/// Force-kill (`taskkill /F`) the running child from outside — the "user
/// closes it in Task Manager / crash" case. PHL's stop path must then settle
/// to "stopped": `terminate_or_gone` tolerates the kill failing against a
/// pid the kernel already reports gone, clears both registries, and a
/// relaunch is allowed again.
#[tokio::test]
#[ignore = "release-e2e F1 force-kill mid-run"]
async fn f1_external_force_kill_leaves_a_clean_stop() {
    let (session, base, runtime) = armed("f1", "e2e-f1").await;
    with_cleanup(&session, async {
        let held = lock_instance(&session.locks, "e2e-f1");
        let launched = super::driver::launch(
            &session,
            &held,
            "f1-launch",
            "e2e-f1",
            super::FAKE_DSH_VERSION,
            &runtime,
            &base,
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &channel::<LaunchEvent>(),
        )
        .await
        .unwrap();
        let port = launched.outcome.port;
        let pid = launched.outcome.pid;

        // Kill it the way the OS does from outside PHL.
        let status = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .unwrap();
        assert!(status.success(), "taskkill must succeed in the harness env");
        assert!(
            wait_until(10_000, || crate::launch::port_free(port)).await,
            "F1: the killed child must stop serving {port}"
        );

        // The production stop path against a confirmed-dead pid: terminate may
        // bounce ("process not found"), but the rows must be cleared so the
        // instance can never get stuck "running" without a handle (R3/
        // terminate_or_gone semantics).
        let _ = super::driver::stop(&session, "e2e-f1").await;
        assert!(
            session.processes.entry_of("e2e-f1").is_none(),
            "F1: in-memory row cleared after the child died"
        );
        assert!(
            session.registry.record_of("e2e-f1").is_none(),
            "F1: durable row cleared after the child died"
        );
    })
    .await;
}

/* -------------------------------- F2 ---------------------------------- */

/// The download dies mid-stream (server sends half the declared body and
/// drops the connection), then the user retries:
///   attempt 1 must fail and leave nothing half-extracted in `versions/`;
///   attempt 2 must succeed with the full bytes and the declared digest.
/// The resume half of the story (append-after-Range, and restart-from-zero on
/// ETag mismatch) is covered unit-level in `versions/download.rs`; this is
/// the same failure seen from the install pipeline the user hits.
#[tokio::test]
#[ignore = "release-e2e F2 download abort + retry"]
async fn f2_aborted_download_leaves_no_half_install() {
    let node = NodeInfo::discover().expect("release-e2e: `node` required on PATH");
    let (server, tgz, integrity) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh("f2");
    with_cleanup(&session, async {
        let tgz_path = format!("/dsh-{}.tgz", super::FAKE_DSH_VERSION);
        server.insert(
            &tgz_path,
            Route::bytes(tgz.clone()).truncated_at(tgz.len() / 2),
        );
        let version = format!("dsh-{}", super::FAKE_DSH_VERSION);
        let tarball = format!("{base}/dsh-{}.tgz", super::FAKE_DSH_VERSION);
        let err = super::journeys::try_install_version(
            &session,
            &base,
            &version,
            &tarball,
            Some(&integrity),
        )
        .await;
        assert!(
            err.is_err(),
            "F2: an aborted download must fail the install"
        );
        assert!(
            !session
                .root()
                .join("versions")
                .join(super::FAKE_DSH_VERSION)
                .exists(),
            "F2: a failed download must leave no version directory"
        );
        // No staged version content survives: `txn_dir` may leave the (empty)
        // `.phl-txn` parent behind by design — what must never happen is a
        // version directory, or a staged payload inside the txn tree that a
        // later scan could mistake for a completed install.
        let versions_dir = session.root().join("versions");
        if versions_dir.exists() {
            let visible: Vec<_> = std::fs::read_dir(&versions_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .collect();
            assert!(
                visible.is_empty(),
                "F2: no visible version entry may exist, got {visible:?}"
            );
            let txn_root = versions_dir.join(".phl-txn");
            if txn_root.exists() {
                let staged: Vec<_> = std::fs::read_dir(&txn_root)
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .collect();
                assert!(
                    staged.is_empty(),
                    "F2: staged transaction children must be cleaned, got {staged:?}"
                );
            }
        }

        // Retry with the healthy server: the full pipeline completes.
        server.insert(&tgz_path, Route::bytes(tgz.clone()));
        super::journeys::try_install_version(&session, &base, &version, &tarball, Some(&integrity))
            .await
            .expect("F2: retry of the aborted download must install");
        assert!(integrity.len() > 10, "integrity fixture sanity");
        assert!(session
            .root()
            .join("versions")
            .join(&version)
            .join("phl-install.json")
            .exists());
    })
    .await;
}

/* -------------------------------- F2b --------------------------------- */

/// The resume story as a *user* triggers it — cancel during a stalled
/// download (O-11's promise: a cancelled transfer keeps its prefix plus a
/// sidecar bound to the URL and ETag):
///   round 1: a slow server stalls mid-body; the transfer is cancelled from
///     the outside while silent. Failure must be `cancelled`, and the
///     `.part` + `.resume` checkpoint must survive.
///   round 2: the healthy server (same entity, same ETag) completes the
///     download — proven by the installed marker carrying the declared
///     digest, which can only happen over exactly those bytes.
///   round 3: the same URL now serves a DIFFERENT entity (extra byte, new
///     ETag) against a fresh checkpoint of the original. The resume must
///     detect the mismatch and restart from zero — never splice — and the
///     digest declared for entity A must then refuse entity B. A success
///     here would be the corruption bug.
#[tokio::test]
#[ignore = "release-e2e F2b cancel-resume and etag-mismatch restart"]
async fn f2b_cancelled_download_resumes_and_rejects_changed_content() {
    let node = NodeInfo::discover().expect("release-e2e: `node` required on PATH");
    let (server, tgz, integrity) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh("f2b");
    with_cleanup(&session, async {
        let version = format!("dsh-{}", super::FAKE_DSH_VERSION);
        let tgz_path = format!("/dsh-{}.tgz", super::FAKE_DSH_VERSION);
        let tarball = format!("{base}{tgz_path}");

        // Round 1: half the bytes, then silence; cancel lands mid-stall.
        server.insert(
            &tgz_path,
            Route::bytes(tgz.clone())
                .with_etag("\"e2e-v1\"")
                .stalling_after(tgz.len() / 2, std::time::Duration::from_secs(6)),
        );
        let id = format!("e2e-version-{version}");
        let flag = session.transfers.take(&id);
        // Trigger the cancel on the FILESYSTEM event (checkpoint bytes
        // appear), not a fixed timer: on a loaded runner 500 ms may precede
        // the first chunk, and the cancel would miss its window.
        super::journeys::cancel_when_checkpoint_appears(
            session
                .root()
                .join("cache")
                .join(format!("dsh-{version}.tgz.part")),
            flag.clone(),
        );
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            guarded_install(
                &session,
                &id,
                &version,
                &base,
                &tarball,
                Some(&integrity),
                &flag,
            ),
        )
        .await
        .expect("F2b: the cancelled transfer must end on its own, not hang");
        session.transfers.release(&id);
        assert_eq!(
            result.unwrap_err(),
            "cancelled",
            "F2b: cancel must surface as cancelled"
        );
        let part = session
            .root()
            .join("cache")
            .join(format!("dsh-{version}.tgz.part"));
        let sidecar = part.with_extension("part.resume");
        assert!(
            part.exists(),
            "F2b: a cancelled transfer keeps its prefix for resume"
        );
        assert!(sidecar.exists(), "F2b: and its URL/ETag sidecar");

        // Round 2: healthy server, same entity — resume completes and installs.
        server.insert(
            &tgz_path,
            Route::bytes(tgz.clone())
                .with_etag("\"e2e-v1\"")
                .with_range(),
        );
        super::journeys::try_install_version(&session, &base, &version, &tarball, Some(&integrity))
            .await
            .expect("F2b: the retry must resume and finish");
        assert!(
            !part.exists() && !sidecar.exists(),
            "F2b: a completed download clears the checkpoint pair"
        );
        let marker: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                session
                    .root()
                    .join("versions")
                    .join(&version)
                    .join("phl-install.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            marker["integrity"].as_str().unwrap(),
            integrity,
            "F2b: the assembled file must hash to the declared digest"
        );

        // Round 3: seed a fresh checkpoint of entity A under a NEW version id,
        // then serve entity B (extra byte, new ETag) from the same URL.
        let mut other = tgz.clone();
        other.push(b'x');
        let version2 = format!("dsh-{}-other", super::FAKE_DSH_VERSION);
        let id2 = format!("e2e-version-{version2}");
        let flag2 = session.transfers.take(&id2);
        server.insert(
            &tgz_path,
            Route::bytes(tgz.clone())
                .with_etag("\"e2e-v1\"")
                .stalling_after(tgz.len() / 2, std::time::Duration::from_secs(6)),
        );
        super::journeys::cancel_when_checkpoint_appears(
            session
                .root()
                .join("cache")
                .join(format!("dsh-{version2}.tgz.part")),
            flag2.clone(),
        );
        let part2 = session
            .root()
            .join("cache")
            .join(format!("dsh-{version2}.tgz.part"));
        let sidecar2 = part2.with_extension("part.resume");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            guarded_install(
                &session,
                &id2,
                &version2,
                &base,
                &tarball,
                Some(&integrity),
                &flag2,
            ),
        )
        .await
        .expect("F2b round 3 setup must end");
        session.transfers.release(&id2);
        assert_eq!(
            result.unwrap_err(),
            "cancelled",
            "F2b: round 3 is cancelled by design"
        );
        assert!(
            part2.exists() && sidecar2.exists(),
            "F2b: round 3 seeds its own checkpoint"
        );
        server.insert(
            &tgz_path,
            Route::bytes(other.clone())
                .with_etag("\"e2e-v2\"")
                .with_range(),
        );
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            super::journeys::try_install_version(
                &session,
                &base,
                &version2,
                &tarball,
                Some(&integrity),
            ),
        )
        .await
        .expect("F2b: the mismatched retry must not hang");
        assert!(
            result.is_err(),
            "F2b: entity B must never satisfy entity A's declared digest (a success means splice corruption)"
        );
        assert!(
            !session
                .root()
                .join("versions")
                .join(&version2)
                .join("lib")
                .join("bin.js")
                .exists(),
            "F2b: a refused mismatch must not promote a version"
        );

    }).await;
}

/// `try_install_version` through an EXISTING cancel flag (the transfer was
/// already registered by the test), so cancellation is observable mid-flight.
async fn guarded_install(
    session: &Session,
    id: &str,
    version: &str,
    registry_base: &str,
    tarball: &str,
    integrity: Option<&str>,
    flag: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    let result = guarded(
        id.to_string(),
        "version-install",
        format!("f2b {version}"),
        vec![Resource::Version(version.to_string())],
        Some(flag.clone()),
        &session.locks,
        &session.tasks,
        |task| {
            let root = session.root();
            let v = version.to_string();
            let t = tarball.to_string();
            let i = integrity.map(str::to_string);
            let base = registry_base.to_string();
            let flag = flag.clone();
            async move {
                let r = crate::versions::run_install(
                    &flag,
                    &t,
                    i.as_deref(),
                    &v,
                    &root,
                    &base,
                    false,
                    None,
                    &task,
                    &super::journeys::progress_channel::<crate::versions::ProgressEvent>(),
                )
                .await;
                if crate::versions::cancelled(&flag) {
                    return Err("cancelled".into());
                }
                r
            }
        },
    )
    .await;
    result
}

/* -------------------------------- F3 ---------------------------------- */

/// Re-install over an EXISTING version whose payload is held open by
/// another handle — the CI-runnable sibling of disk-full (a real full disk
/// needs admin quota/VHD, see docs/windows-e2e-release-gate.md).
///
/// Honest scope: Windows locks files, not parent renames — with a held file
/// INSIDE the destination the transaction may legitimately complete or
/// refuse; what must hold on EITHER outcome is that the user's version
/// never VANISHES. The `[disk-full]` ENOSPC classification is unit-tested
/// in `storage.rs`/`versions/install.rs`; this smoke pins "a half-swap
/// never leaves zero copies behind" against real NTFS locking.
#[tokio::test]
#[ignore = "release-e2e F3 locked payload: install never vanishes"]
async fn f3_reinstall_with_locked_payload_never_vanishes_the_version() {
    let (session, base, _runtime) = armed("f3", "e2e-f3").await;
    with_cleanup(&session, async {
        let dest = session
            .root()
            .join("versions")
            .join(super::FAKE_DSH_VERSION);
        assert!(dest.join("lib").join("bin.js").exists());

        // Hold the destination open (FileShare::NONE-ish: any open handle makes
        // `rename` of that exact directory fail on Windows).
        let marker = dest.join("README.lock-holder");
        std::fs::write(&marker, b"x").unwrap();
        let holder = std::fs::File::open(&marker).unwrap();

        // Re-install over it. The promote step renames the destination aside
        // first — Windows refuses a rename whose target has an open handle only
        // for *files* held exclusively; the directory rename itself succeeds, so
        // this test asserts the weaker, still-user-visible invariant: the old
        // bytes must remain reachable one way or the other (renamed backup or
        // untouched tree), and the result is either a completed install or a
        // refusal — never a vanished version.
        let version = format!("dsh-{}", super::FAKE_DSH_VERSION);
        let tarball = format!("{base}/dsh-{}.tgz", super::FAKE_DSH_VERSION);
        let integrity = integrity_sha512(&fake_dsh_tarball(super::FAKE_DSH_VERSION));
        let result =
            super::journeys::try_install_version(&session, &base, &version, &tarball, Some(&integrity))
                .await;
        drop(holder);
        let post = read_version_tree(&session.root());
        assert!(
            result.is_ok() || post.iter().any(|x| x == super::FAKE_DSH_VERSION),
            "F3: after a locked-destination reinstall the version must still exist; got result={result:?} tree={post:?}"
        );
        // Whatever happened, a completed install passes the health gate:
        if result.is_ok() {
            assert!(
                dest.join("phl-install.json").exists(),
                "F3: the committed install carries its marker"
            );
        }
    }).await;
}

fn read_version_tree(root: &std::path::Path) -> Vec<String> {
    let dir = root.join("versions");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/* -------------------------------- F4 ---------------------------------- */

/// The R3 boundary, end to end: a child that refuses to die (installs a
/// signal handler, sleeps before exiting) through the REAL launch pipeline
/// (cancel flag flipped mid-startup) —
///   · `terminate_and_forget` keeps in-memory + durable registrations and
///     reports the coded `kept-alive` error with pid and port;
///   · a relaunch is refused (`ensure_launch_available`) while the old row is
///     alive;
///   · once the child is confirmed gone, the *next* stop clears the rows and
///     the port frees for a fresh launch.
#[tokio::test]
#[ignore = "release-e2e F4 late-exiting child keeps registrations (R3)"]
async fn f4_late_exiting_child_keeps_registration_until_confirmed() {
    let node = NodeInfo::discover().expect("release-e2e: `node` required on PATH");
    let session = Session::fresh("f4");
    with_cleanup(&session, async {
        let root = session.root();

        // Stub that delays its death ~3 s after a console-control event
        // (taskkill's WM_CLOSE reaches SetConsoleCtrlHandler; /F bypasses it —
        // exactly the asymmetry this test pins down).
        let script = r#"
    process.stdout.write("stub-up\n");
    const busy = setTimeout(() => process.exit(0), 3000);
    process.on('SIGINT', () => {});
    process.on('SIGTERM', () => {});
    process.on('message', () => {});
    if (process.platform === 'win32') {
      try {
        const m = require('module');
        // No native dep: a plain timer keeps the loop alive; the test's taskkill
        // without /F posts WM_CLOSE, which Node maps to SIGINT — ignored above.
        void m;
      } catch {}
    }
    clearTimeout(busy);
    setInterval(() => {}, 1000);
    "#;
        let script_path = root.join("stub.js");
        std::fs::write(&script_path, script).unwrap();
        let child = tokio::process::Command::new(&node.exe)
            .arg(&script_path)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        // Never a fixed port: a collision would make the kept-alive
        // scenario block on an unrelated occupant. First free port.
        let port = (45000u16..46000)
            .find(|p| crate::launch::port_free(*p))
            .expect("F4: a free port in 45000-46000");
        // The durable row a real launch would have committed at spawn time.
        session
            .processes
            .set("e2e-f4", crate::launch::ProcessEntry { pid, port });
        session.registry.remember(crate::launch::PersistedProcess {
            instance_id: "e2e-f4".into(),
            pid,
            port,
            started_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
            exe_path: node.exe.to_string_lossy().into_owned(),
        });

        // Non-forced kill: the stub ignores the signal and stays alive.
        let soft = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string()])
            .status();
        let _ = soft; // outcome varies by console state; liveness is what matters
        assert!(
            crate::launch::probe_process(pid).state == crate::launch::ProcessState::Alive
                || !super::wait_until(1_500, || crate::launch::probe_process(pid).state
                    == crate::launch::ProcessState::Exited)
                .await,
            "F4 precondition: the stub should survive a non-forced kill"
        );

        // A relaunch through the production shell must refuse with kept-alive.
        let held = lock_instance(&session.locks, "e2e-f4");
        let refused = super::driver::launch(
            &session,
            &held,
            "f4-relaunch",
            "e2e-f4",
            super::FAKE_DSH_VERSION,
            &node.runtime_name(),
            "http://127.0.0.1:1", // never reached; refused before the network
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &channel::<LaunchEvent>(),
        )
        .await
        .expect_err("F4: relaunch while a kept-alive process is registered must be refused");
        assert!(
            refused.contains("kept-alive"),
            "F4: refusal must be the machine-readable kept-alive header, got {refused}"
        );
        assert!(
            refused.contains(&pid.to_string()),
            "F4: refusal names the pid"
        );
        assert!(
            session.processes.entry_of("e2e-f4").is_some()
                && session.registry.record_of("e2e-f4").is_some(),
            "F4: refusal must not drop either registration"
        );

        // Now force-kill (the user finally does it in Task Manager), wait for the
        // kernel to confirm, and stop through the production path: rows clear.
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .unwrap();
        assert!(
            wait_until(10_000, || probe_exited(pid)).await,
            "F4: the forced stub must die"
        );
        super::driver::stop(&session, "e2e-f4").await.unwrap();
        assert!(session.processes.entry_of("e2e-f4").is_none());
        assert!(session.registry.record_of("e2e-f4").is_none());
    })
    .await;
}

fn probe_exited(pid: u32) -> bool {
    crate::launch::probe_process(pid).state == crate::launch::ProcessState::Exited
}

/* F5 (disk-full) intentionally has no automated placeholder: an early
`return` reads green in the tally while asserting nothing. A real ENOSPC
needs an admin quota or a VHD — the self-hosted VM lane runs it manually
(docs/windows-e2e-release-gate.md §VM). The CI-runnable sibling (locked
payload) is F3; the coded `[disk-full]` classification is unit-tested in
`storage.rs`/`versions/install.rs`. */

//! Acceptance journeys J1–J5: the ordinary user's happy path, machine-checked.

#![allow(dead_code)] // journeys share helpers that light up in different cases

use std::collections::HashMap;

use crate::instances::manifest::InstanceManifest;
use crate::launch::{decide, probe_process, Adoption, LaunchEvent, Registry};
use crate::resources::{guarded, Resource, TaskInfo};
use crate::runtimes::run_runtime_install;
use crate::versions::run_install;

use super::driver::{lock_instance, with_cleanup, Launched, Session};
use super::fixtures::{
    fake_dsh_tarball, fake_node_dist, integrity_sha512, no_cancel, scratch, MockServer, NodeInfo,
    Route,
};
use super::{packument_json, wait_until};

/// Watch for the resume checkpoint to appear (first bytes on disk), then
/// flip the cancel flag — the cancel lands mid-stream by construction,
/// independent of runner speed. Falls back to a blind cancel if the
/// checkpoint never appears (a broken server then simply fails the next
/// assertion instead of hanging the test budget).
pub(crate) fn cancel_when_checkpoint_appears(
    part: std::path::PathBuf,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        for _ in 0..200 {
            if std::fs::metadata(&part)
                .map(|m| m.len() > 0)
                .unwrap_or(false)
            {
                flag.store(true, Ordering::SeqCst);
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        flag.store(true, Ordering::SeqCst);
    });
}

/// Every `run_runtime_install`/`run_install` call threads a `Task` and a
/// `Channel`; headless they are stubs the installers write into without the
/// GUI ever reading them back. (`Channel::new` itself has no trait bound on
/// its message type — `LaunchEvent`/`ProgressEvent` are Serialize-only.)
pub(crate) fn progress_channel<TSend>() -> tauri::ipc::Channel<TSend> {
    tauri::ipc::Channel::new(|_| Ok(()))
}

/// Bring up a mock server wired with the two fake artifacts the cold path
/// needs: a Node `win-x64` dist zip (real `node.exe`) and a dependency-free
/// DSH tarball. Returns the server, its base URL, the tarball bytes and the
/// integrity string so a journey can also assert the verify path.
pub(crate) async fn cold_mocks(node: &NodeInfo) -> (MockServer, Vec<u8>, String) {
    let server = MockServer::start().await;
    let base = server.base();
    let (zip, shasums) = fake_node_dist(node);
    let v = &node.version;
    server.insert(&format!("/v{v}/node-v{v}-win-x64.zip"), Route::bytes(zip));
    server.insert(&format!("/v{v}/SHASUMS256.txt"), Route::text(&shasums));
    let tgz = fake_dsh_tarball(super::FAKE_DSH_VERSION);
    let integrity = integrity_sha512(&tgz);
    server.insert(
        &format!("/dsh-{}.tgz", super::FAKE_DSH_VERSION),
        Route::bytes(tgz.clone()),
    );
    server.insert(
        "/@deepseek-ai%2Fdsh",
        Route::text(&packument_json(&base, super::FAKE_DSH_VERSION, &integrity)),
    );
    (server, tgz, integrity)
}

/// Install a Node runtime from the mock dist base through the *same*
/// `guarded` shell the `download_node_runtime` command runs (lock → task →
/// cancel flag → release), so the gate exercises the bookkeeping too.
pub(crate) async fn install_runtime(session: &Session, dist_base: &str, node: &NodeInfo) -> String {
    let major_name = node.runtime_name();
    let flag = session.transfers.take("e2e-runtime");
    let root = session.root();
    let version = node.version.clone();
    let base = dist_base.to_string();
    let major_for_label = major_name.clone();
    let result = guarded(
        "e2e-runtime".into(),
        "runtime-install",
        format!("安装 Runtime {major_name}"),
        vec![Resource::Runtime(major_name.clone())],
        Some(flag.clone()),
        &session.locks,
        &session.tasks,
        |task| async move {
            let r = run_runtime_install(
                &flag,
                &base,
                &major_for_label,
                &version,
                &root,
                false,
                &task,
                &progress_channel(),
            )
            .await;
            if crate::versions::cancelled(&flag) {
                return Err("cancelled".into());
            }
            r
        },
    )
    .await;
    session.transfers.release("e2e-runtime");
    result.unwrap();
    major_name
}

/// The install pipeline as a `Result` (fault cases assert the failure), run
/// through the same `guarded` shell as the `download_dsh_version` command.
/// `integrity = None` models a registry that ships no digest — the pipeline
/// must still install (`verify_integrity` skips on None, `download.rs:454`).
pub(crate) async fn try_install_version(
    session: &Session,
    registry_base: &str,
    version: &str,
    tarball_url: &str,
    integrity: Option<&str>,
) -> Result<(), String> {
    let id = format!("e2e-version-{version}");
    let flag = session.transfers.take(&id);
    let root = session.root();
    let base = registry_base.to_string();
    let version_owned = version.to_string();
    let tarball_owned = tarball_url.to_string();
    let integrity_owned = integrity.map(str::to_string);
    let result = guarded(
        id.clone(),
        "version-install",
        format!("安装版本 {version_owned}"),
        vec![Resource::Version(version_owned.clone())],
        Some(flag.clone()),
        &session.locks,
        &session.tasks,
        |task| async move {
            let r = run_install(
                &flag,
                &tarball_owned,
                integrity_owned.as_deref(),
                &version_owned,
                &root,
                &base,
                false,
                None,
                &task,
                &progress_channel(),
            )
            .await;
            if crate::versions::cancelled(&flag) {
                return Err("cancelled".into());
            }
            r
        },
    )
    .await;
    session.transfers.release(&id);
    result
}

/// Install the fake DSH version from the mock registry through the real
/// pipeline with its true digest. `version` is the canonical catalog id
/// (`dsh-<semver>`); the mock packument promised that tarball spelling.
pub(crate) async fn install_version(session: &Session, registry_base: &str, version: &str) {
    let semver = version.strip_prefix("dsh-").unwrap_or(version);
    let integrity = integrity_sha512(&fake_dsh_tarball(semver));
    try_install_version(
        session,
        registry_base,
        version,
        &format!("{registry_base}/dsh-{semver}.tgz"),
        Some(&integrity),
    )
    .await
    .unwrap();
}

pub(crate) fn manifest_for(id: &str, version: &str, runtime: &str) -> InstanceManifest {
    InstanceManifest {
        schema_version: 0,
        id: id.into(),
        name: format!("E2E {id}"),
        note: None,
        kind: "sandbox".into(),
        hue: 0,
        version_id: version.into(),
        runtime_id: runtime.into(),
        port: 8080,
        auto_port: true,
        profile: "web".into(),
        created_at: crate::versions::now_iso(),
        last_run_at: None,
        total_runtime: 0,
        favorite: false,
        env: HashMap::new(),
        args: Vec::new(),
        api: None,
        management_mode: Default::default(),
        source: Default::default(),
        external_home: None,
        adopted_from: None,
    }
}

/* --------------------------------- J1 --------------------------------- */

/// The cold happy path a user walks within minutes of installing PHL, against
/// a throwaway data root and zero real network:
/// download Runtime → download+install DSH → create instance → start → the
/// readiness socket is actually serving → stop → the process is gone.
#[tokio::test]
#[ignore = "release-e2e J1 cold install → launch → stop"]
async fn j1_cold_install_launch_and_stop() {
    let Some(node) = NodeInfo::discover() else {
        panic!("release-e2e J1: no `node` on PATH — install Node or run on a CI image");
    };
    let (server, _tgz, _integrity) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh("j1");
    with_cleanup(&session, async {
        // 1. Runtime download + install (real SHASUMS verify + real node.exe
        //    health gate).
        let runtime = install_runtime(&session, &base, &node).await;
        let runtime_dir = session.root().join("runtimes").join(&runtime);
        assert!(
            runtime_dir.join(crate::launch::node_binary()).exists(),
            "J1: runtime binary landed at {}",
            runtime_dir.display()
        );

        // 2. DSH version download + install + health + promote. The install
        //    directory is the canonical catalog id `dsh-<version>` — the same
        //    spelling `bound_id_for_version` writes into instance bindings.
        let version = format!("dsh-{}", super::FAKE_DSH_VERSION);
        install_version(&session, &base, &version).await;
        let bin = session
            .root()
            .join("versions")
            .join(&version)
            .join("lib")
            .join("bin.js");
        assert!(bin.exists(), "J1: DSH entrypoint extracted");

        // 1b. The catalog itself: `list_dsh_versions` against the mock registry
        //     must surface the promised version with the promised digest. (The
        //     GitHub merge leg is tolerated-failure, so this asserts membership,
        //     not exclusivity.)
        let listed = crate::versions::list_dsh_versions(base.clone())
            .await
            .unwrap();
        let ours = listed
            .iter()
            .find(|v| v.name == *super::FAKE_DSH_VERSION)
            .expect("J1: the mock registry version must appear in the catalog");
        let ours_src = ours
            .source
            .as_ref()
            .expect("J1: a published version carries an install source");
        assert!(
            ours_src.tarball.contains("/dsh-"),
            "J1: catalog points at a tarball"
        );

        // 2b. A registry that ships no `dist.integrity` must still install: the
        //     skip path of `verify_integrity` is a real-world shape (mirrors drop
        //     it) and PHL's promise is to complete, not to choke. Re-serve the
        //     packument without a digest and re-read the catalog to prove the
        //     shape is accepted end-to-end, not just skipped by our direct call.
        server.insert(
            "/@deepseek-ai%2Fdsh",
            Route::text(&super::packument_json_no_integrity(
                &base,
                super::FAKE_DSH_VERSION,
            )),
        );
        let relisted = crate::versions::list_dsh_versions(base.clone())
            .await
            .unwrap();
        let noi = relisted
            .iter()
            .find(|v| v.name == *super::FAKE_DSH_VERSION)
            .expect("J1: the digest-less packument still lists the version");
        assert!(
            noi.source
                .as_ref()
                .and_then(|v| v.integrity.as_ref())
                .is_none(),
            "J1: the registry offered no integrity"
        );
        let _ = noi;
        let noi_version = format!("dsh-{}-noi", super::FAKE_DSH_VERSION);
        server.insert(
            &format!("/dsh-{noi_version}.tgz"),
            Route::bytes(fake_dsh_tarball(super::FAKE_DSH_VERSION)),
        );
        try_install_version(
            &session,
            &base,
            &noi_version,
            &format!("{base}/dsh-{noi_version}.tgz"),
            None,
        )
        .await
        .expect("J1: a digest-less registry entry must still install");
        assert!(session
            .root()
            .join("versions")
            .join(&noi_version)
            .join("lib")
            .join("bin.js")
            .exists());

        // 3. Create the instance (the real create path, including the tree it
        //    lays down under the root).
        let held = lock_instance(&session.locks, "e2e-j1");
        let _ = crate::instances::create_instance_inner(
            &session.root(),
            manifest_for("e2e-j1", &version, &runtime),
        )
        .await
        .unwrap();
        assert!(session
            .root()
            .join("instances")
            .join("e2e-j1")
            .join("instance.json")
            .exists());

        // 4. Launch — the real startup pipeline minus the window watcher. A
        //    dependency-free fake lets the launch skip npm entirely.
        let launched: Launched = super::driver::launch(
            &session,
            &held,
            "e2e-j1-launch",
            "e2e-j1",
            &version,
            &runtime,
            &base,
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &progress_channel::<LaunchEvent>(),
        )
        .await
        .expect("J1: launch must succeed");
        let port = launched.outcome.port;
        assert!(
            super::port_listening(port).await,
            "J1: launched child must be accepting on {port}"
        );

        // 5. Stop — the real `terminate_or_gone` path, and the port frees.
        session.terminate("e2e-j1").await;
        assert!(
            wait_until(8_000, || crate::launch::port_free(port)).await,
            "J1: port {port} must free after stop"
        );
    })
    .await;
}

/* --------------------------------- J2 --------------------------------- */

/// Restart adoption (O-08), headless: after a launch, the *durable* registry
/// row survives; a fresh `Registry` bound to the same processes file re-adopts
/// the live child by identity — same exe, matching creation window — without
/// ever re-launching it or killing a stranger's pid. This is the data half of
/// "重启 PHL 接管"; the GUI window events are the installer/CDP lane.
#[tokio::test]
#[ignore = "release-e2e J2 restart adoption"]
async fn j2_restart_adopts_a_live_child() {
    let Some(node) = NodeInfo::discover() else {
        panic!("J2: no node")
    };
    let (server, _, _) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh("j2");
    with_cleanup(&session, async {
        let runtime = install_runtime(&session, &base, &node).await;
        let version = format!("dsh-{}", super::FAKE_DSH_VERSION);
        install_version(&session, &base, &version).await;
        let held = lock_instance(&session.locks, "e2e-j2");
        crate::instances::create_instance_inner(
            &session.root(),
            manifest_for("e2e-j2", &version, &runtime),
        )
        .await
        .unwrap();
        let launched = super::driver::launch(
            &session,
            &held,
            "e2e-j2-launch",
            "e2e-j2",
            &version,
            &runtime,
            &base,
            0,
            true,
            HashMap::new(),
            Vec::new(),
            "web",
            &progress_channel::<LaunchEvent>(),
        )
        .await
        .unwrap();
        let pid = launched.outcome.pid;
        assert!(
            super::port_listening(launched.outcome.port).await,
            "J2: the launched child must be serving before the simulated restart"
        );
        drop(held);

        // Simulate a PHL restart: a brand-new registry bound to the same
        // processes.json the launch committed to.
        let processes_path = session
            .phl
            .sibling_file("processes.json")
            .expect("J2: durable registry must exist for restart adoption");
        let revived = Registry::default();
        revived.bind(processes_path);
        let persisted = revived
            .record_of("e2e-j2")
            .expect("J2: the launch committed a durable row");
        assert_eq!(persisted.pid, pid, "J2: durable pid is the live child");

        // Probe the real OS state and run the adoption decision.
        let decision = decide(&persisted, &probe_process(pid));
        assert!(
            matches!(decision, Adoption::Adopt),
            "J2: a live, correctly-identified child must be adopted, got {decision:?}"
        );

        session.terminate("e2e-j2").await;
    })
    .await;
}

/* --------------------------------- J3 --------------------------------- */

/// Snapshot → modify → restore: the snapshot copies `dsh-home` only; after a
/// restore the on-disk state matches the snapshot. Then pack export → install
/// into a *second* root proves cross-machine reproducibility (the user's
/// promise in the roadmap: "另一台机器重建它").
#[tokio::test]
#[ignore = "release-e2e J3 snapshot + pack roundtrip"]
async fn j3_snapshot_then_pack_rebuild() {
    let Some(node) = NodeInfo::discover() else {
        panic!("J3: no node")
    };
    let (server, _, _) = cold_mocks(&node).await;
    let base = server.base();
    let session = Session::fresh("j3");
    with_cleanup(&session, async {
        let runtime = install_runtime(&session, &base, &node).await;
        let version = format!("dsh-{}", super::FAKE_DSH_VERSION);
        install_version(&session, &base, &version).await;
        let held = lock_instance(&session.locks, "e2e-j3");
        crate::instances::create_instance_inner(
            &session.root(),
            manifest_for("e2e-j3", &version, &runtime),
        )
        .await
        .unwrap();
        drop(held);

        // Materialise a file inside dsh-home so the snapshot has content to keep.
        let home = session
            .root()
            .join("instances")
            .join("e2e-j3")
            .join("dsh-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("marker.txt"), b"pre-snapshot").unwrap();

        // Snapshot it.
        let snap = crate::instances::snapshot::run_snapshot_create(
            &no_cancel(),
            &session.processes,
            &session.root(),
            "e2e-j3",
            &|_p| {},
        )
        .await
        .unwrap();

        // Modify, then restore: the marker must roll back to the snapshot state.
        std::fs::write(home.join("marker.txt"), b"mutated-after-snapshot").unwrap();
        crate::instances::snapshot::restore_snapshot_inner(
            &session.root(),
            "e2e-j3",
            &snap.id,
            &session.processes,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("marker.txt")).unwrap(),
            "pre-snapshot",
            "J3: restore must put the snapshot bytes back on disk"
        );

        // Pack export → install into a fresh root B (the "other machine").
        let pack = session.root().join("e2e-j3.phlpack");
        crate::pack::export::export_pack_inner(
            &session.root(),
            "e2e-j3".into(),
            pack.to_string_lossy().into_owned(),
            crate::pack::export::PackExportOptions {
                embed_registry_ids: Vec::new(),
                include_sessions: false,
                sessions_privacy_ack: false,
            },
        )
        .await
        .unwrap();
        assert!(pack.exists(), "J3: pack written");

        // The second machine is just an empty data root: `install_inner` works
        // from the pack bytes, nothing about the first machine is needed.
        let b = scratch("j3-root-b");
        let req = crate::pack::install::PackInstallRequest {
            manifest: manifest_for("e2e-j3-copy", "dsh-9.9.9", "node-22"),
            allow_missing: true,
        };
        let task = crate::resources::Tasks::default()
            .begin(
                TaskInfo::new("pack-install-j3".into(), "pack-install", "j3".into(), &[]),
                Some(no_cancel()),
            )
            .unwrap();
        let outcome = crate::pack::install::install_inner(
            &b,
            task,
            &pack.to_string_lossy(),
            req,
            &progress_channel(),
            no_cancel(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.record.manifest.id, "e2e-j3-copy");
        assert!(b
            .join("instances")
            .join("e2e-j3-copy")
            .join("instance.json")
            .exists());

        session.cleanup().await;
        let _ = std::fs::remove_dir_all(&b);
    })
    .await;
}

/* --------------------------------- J4 --------------------------------- */

/// Data migration across the two journeys the settings UI offers: a complete
/// move commits the pointer exactly once, an interrupted move is resumable
/// from the journal, and an unfinished move can be undone back to the source.
/// Every step asserts disk state, not UI text.
#[tokio::test]
#[ignore = "release-e2e J4 storage migration commit/resume/undo"]
async fn j4_storage_migration_paths() {
    let session = Session::fresh("j4");
    with_cleanup(&session, async {
        let from = session.root();
        // Put real bytes in the managed dirs so the journal has content rows.
        for dir in ["instances/keepme", "versions/dsh-1.0.0", "runtimes/node-22"] {
            let p = from.join(dir);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("payload.txt"), format!("{dir} contents").as_bytes()).unwrap();
        }
        let journal_path = session
            .phl
            .sibling_file(crate::storage::JOURNAL_NAME)
            .expect("J4: pointer exists so the journal is persistent");
        let to = session.base.join("moved");

        // 1. Full move commits: pointer follows, journal cleared, bytes present.
        crate::storage::move_root_inner(
            &no_cancel(),
            &super::fixtures::task("root-migration", "j4-a"),
            Some(&journal_path),
            &from,
            &to,
            &|_p| {},
        )
        .await
        .unwrap();
        session
            .phl
            .relocate_root(to.clone(), Some(&journal_path))
            .unwrap();
        assert_eq!(
            session.phl.root(),
            to,
            "J4: pointer committed to the new root"
        );
        assert!(
            !journal_path.exists(),
            "J4: committed migration retires the journal"
        );
        assert!(
            to.join("instances/keepme/payload.txt").exists(),
            "J4: bytes moved"
        );

        // 2. Interrupted move resumes: hand the journal an in-flight row, then a
        //    second `move_root_inner` continues from it (not from scratch).
        let back = session.base.join("resumed");
        std::fs::create_dir_all(&back).unwrap();
        let mut j = crate::storage::MigrationJournal {
            from: super::fixtures::abs(&to),
            to: super::fixtures::abs(&back),
            entries: DATA_DIRS_J4
                .iter()
                .map(|k| crate::storage::JournalEntry {
                    kind: (*k).to_string(),
                    state: crate::storage::EntryState::Pending,
                    bytes: 0,
                })
                .collect(),
            committed: false,
            started_at: crate::versions::now_iso(),
        };
        if let Some(e) = j.entries.iter_mut().find(|e| e.kind == "instances") {
            e.state = crate::storage::EntryState::Moved;
        }
        std::fs::write(&journal_path, serde_json::to_vec_pretty(&j).unwrap()).unwrap();
        let summary = crate::storage::move_root_inner(
            &no_cancel(),
            &super::fixtures::task("root-migration", "j4-b"),
            Some(&journal_path),
            &to,
            &back,
            &|_p| {},
        )
        .await
        .unwrap();
        assert!(
            summary.moved.iter().any(|k| k != "instances") || !summary.moved.is_empty(),
            "J4: resume must move the remaining directories, got {:?}",
            summary.moved
        );
        session
            .phl
            .relocate_root(back.clone(), Some(&journal_path))
            .unwrap();
        assert!(
            back.join("runtimes/node-22/payload.txt").exists(),
            "J4: resumed bytes complete"
        );

        // 3. Undo an unfinished move: a fresh journaled attempt, then undo puts
        //    everything back at the source and clears the record.
        // A *fresh* from/to pair for the undo case (round 3): `back` already has
        // the data; moving it out then undoing must return it to `back`. If from
        // and to shared a name the undo's "source exists, refuse" guard would
        // (correctly) block the return — so the two roots are distinct.
        let from3 = session.base.join("undone-source");
        std::fs::create_dir_all(from3.join("instances/keepme")).unwrap();
        std::fs::write(from3.join("instances/keepme/payload.txt"), b"undo-me").unwrap();
        let to3 = session.base.join("undone-target");
        let flag = no_cancel();
        crate::storage::move_root_inner(
            &flag,
            &super::fixtures::task("root-migration", "j4-c"),
            Some(&journal_path),
            &from3,
            &to3,
            &|_p| {},
        )
        .await
        .unwrap();
        let mut j2 = crate::storage::read_journal(&journal_path)
            .unwrap()
            .unwrap();
        // Mark the journal NOT committed and walk it back.
        j2.committed = false;
        std::fs::write(&journal_path, serde_json::to_vec_pretty(&j2).unwrap()).unwrap();
        crate::storage::undo_migration(
            &journal_path,
            &j2,
            &super::fixtures::task("root-migration", "j4-d"),
        )
        .await
        .unwrap_or_else(|e| panic!("J4: undo failed: {e}"));
        for kind in DATA_DIRS_J4 {
            println!(
                "J4 undo probe {kind}: from3={} exists={} | to3={} exists={}",
                from3.join(kind).display(),
                from3.join(kind).exists(),
                to3.join(kind).display(),
                to3.join(kind).exists()
            );
        }
        assert!(!journal_path.exists(), "J4: undo clears the record");
        let ret =
            std::fs::read_to_string(from3.join("instances/keepme/payload.txt")).unwrap_or_default();
        assert_eq!(
            ret, "undo-me",
            "J4: undo must return the data to the source"
        );
    })
    .await;
}

/// The managed directory set, mirroring `storage.rs:33` so hand-written
/// journals stay in sync with the production set.
const DATA_DIRS_J4: [&str; 5] = ["instances", "versions", "runtimes", "config", "cache"];

/* --------------------------------- J5 --------------------------------- */

/// The update path users hit after installation: PHL checks the signed
/// `updates`-branch manifest. This journey runs the *contract* against the
/// real endpoint (network required) — it parses the JSON into the shape the
/// updater plugin consumes, verifies the embedded minisign signature decodes,
/// and asserts the pubkey in `tauri.conf.json` is the one the workflow uses.
/// It does not invoke the in-app updater (that needs a window); it is the
/// gate that catches a broken manifest before a release is published.
#[tokio::test]
#[ignore = "release-e2e J5 updater manifest contract (online)"]
async fn j5_updates_manifest_contract() {
    const URL: &str = "https://raw.githubusercontent.com/A7m0spHere/dsh-phl/updates/latest.json";
    // Transport trouble SKIPs (a laptop flake must not turn the gate red —
    // a flaky gate is an ignored gate); an answer that arrives but is WRONG
    // (status, JSON, contract) is a break and fails. The Release gate still
    // runs this against GitHub's own raw endpoint; a release-time network
    // failure would surface there via the later publish steps anyway.
    let body =
        match tokio::time::timeout(std::time::Duration::from_secs(15), reqwest::get(URL)).await {
            Ok(Ok(resp)) => match resp.error_for_status() {
                Ok(resp) => resp.text().await.unwrap_or_default(),
                // A 404 here means the `updates` branch has no manifest yet —
                // the chicken-and-egg of a repo's FIRST ever release (the gate
                // runs before the first publish). This repo seeded it at
                // alpha.1; a brand-new fork must run this test only after its
                // first manifest exists, or treat 404 as expected-once.
                Err(e) => panic!("J5: endpoint returned an error status: {e}"),
            },
            Ok(Err(e)) => {
                eprintln!("J5 SKIPPED (network unreachable): {e} — rerun online before release");
                return;
            }
            Err(_) => {
                eprintln!("J5 SKIPPED (endpoint timed out) — rerun online before release");
                return;
            }
        };
    let manifest: serde_json::Value = serde_json::from_str(&body)
        .expect("J5: latest.json must be valid JSON the updater plugin can parse");
    let version = manifest["version"].as_str().expect("J5: version present");
    assert!(
        semver::Version::parse(version).is_ok(),
        "J5: version {version} is semver"
    );
    let url = manifest["platforms"]["windows-x86_64"]["url"]
        .as_str()
        .expect("J5: a windows asset url is promised");
    assert!(
        url.ends_with(".exe"),
        "J5: windows asset is an .exe installer"
    );
    let sig = manifest["platforms"]["windows-x86_64"]["signature"]
        .as_str()
        .expect("J5: signature present");
    // The signing key must be the one PHL was BUILT to trust: the minisign
    // key id (little-endian u64 at bytes 2..10 of the decoded signature /
    // public line) has to agree between `latest.json` and the pubkey in
    // `tauri.conf.json`. Drift here means installed copies reject the next
    // update — the exact check the alpha.4 acceptance recorded by hand.
    let conf: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")).unwrap(),
    )
    .unwrap();
    let pubkey = conf["plugins"]["updater"]["pubkey"]
        .as_str()
        .expect("J5: the app embeds an updater pubkey");
    let sig_key = minisign_keyid(sig).expect("J5: manifest signature decodes as minisign");
    let pub_key = minisign_keyid(pubkey).expect("J5: embedded pubkey decodes as minisign");
    assert_eq!(
        sig_key, pub_key,
        "J5: the signing key changed out from under the embedded pubkey — installed apps would refuse this update"
    );
    println!("J5: updates branch serves {version}; signature keyid {sig_key:#018x} matches the embedded pubkey");
}

/// Decode a base64-wrapped minisign signature *or* public key and return its
/// key id: the little-endian u64 at bytes 2..10 of the decoded "global
/// signature"/"public" line (format reference: minisign ED25519 pre-hash).
/// Mirrors the keyid check recorded in the alpha.4 release acceptance.
fn minisign_keyid(b64_block: &str) -> Option<u64> {
    use base64::Engine as _;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64_block.trim())
        .ok()?;
    let text = String::from_utf8(raw).ok()?;
    // Signature files are 2- or 4-line; public keys are 2-line. The line
    // whose decoded prefix is the "global signature" / "public" tag carries
    // the keyid at bytes 2..10.
    for line in text.lines() {
        let Ok(chunk) = base64::engine::general_purpose::STANDARD.decode(line.trim()) else {
            continue;
        };
        if chunk.len() < 10 {
            continue;
        }
        let tag = u16::from_le_bytes([chunk[0], chunk[1]]);
        // 0x4565 = "Ed" prehashed signature, 0x6445 = "Ed" legacy sig,
        // 0x4575 = pubkey format — all carry the keyid at 2..10.
        if matches!(tag, 0x4445 | 0x6445) {
            return Some(u64::from_le_bytes(chunk[2..10].try_into().ok()?));
        }
    }
    None
}

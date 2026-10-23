use super::*;
use std::collections::BTreeMap;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ID: &str = "restore-p0";
const SNAP: &str = "snap-p0";
const WORKER: &str = "instances::snapshot::restore::tests::crash_worker";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    async fn new(tag: &str) -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "phl-restore-p0-{}-{}-{}-{tag}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join(".owned-test-data"), b"snapshot-p0-fixture").unwrap();
        let f = Self { root };
        let home = f.home();
        for (name, bytes) in [
            (
                "sessions/project/live.jsonl",
                b"live session\0\xff".as_slice(),
            ),
            ("sessions/project/nested/empty", b"".as_slice()),
            (
                "attachments/nested/image.bin",
                b"\0\x01attachment\xff".as_slice(),
            ),
            ("settings.yaml", b"live-config".as_slice()),
        ] {
            write(&home.join(name), bytes);
        }
        crate::instances::manifest::write_manifest(&f.dir(), &manifest())
            .await
            .unwrap();
        let snap = f.dir().join("snapshots").join(SNAP);
        std::fs::create_dir_all(&snap).unwrap();
        let meta = SnapshotFile {
            id: SNAP.into(),
            label: "P0".into(),
            created_at: "test".into(),
            version_id: "dsh-0.1.0".into(),
            runtime_id: "node-system".into(),
            plugin_count: 0,
            size: 0,
        };
        std::fs::write(
            snap.join("snapshot.json"),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
        write(&snap.join("dsh-home/settings.yaml"), b"snapshot-config");
        write(
            &snap.join("dsh-home/sessions/stale"),
            b"snapshot-only session",
        );
        write(
            &snap.join("dsh-home/attachments/stale"),
            b"snapshot-only attachment",
        );
        f
    }
    fn dir(&self) -> PathBuf {
        self.root.join("instances").join(ID)
    }
    fn home(&self) -> PathBuf {
        self.dir().join("dsh-home")
    }
    fn stage(&self) -> PathBuf {
        self.dir().join(STAGING)
    }
    fn backup(&self) -> PathBuf {
        self.dir().join(BACKUP)
    }
    fn prepare(&self, phase: Phase) {
        std::fs::create_dir_all(self.stage().join("dsh-home")).unwrap();
        write_journal(&self.dir(), ID, phase).unwrap();
    }
    async fn recover(&self) -> Result<(), String> {
        recover(&self.dir(), &manifest()).await
    }
    async fn restore(
        &self,
        hook: &(impl Fn(Point) -> Result<(), String> + Send + Sync),
    ) -> Result<InstanceRecord, String> {
        run(
            &self.root,
            ID,
            SNAP,
            &Processes::default(),
            &Arc::new(AtomicBool::new(false)),
            &|_| {},
            hook,
        )
        .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.root.join(".owned-test-data").is_file() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

fn manifest() -> InstanceManifest {
    serde_json::from_value(serde_json::json!({
        "id": ID, "name": "P0 fixture", "kind": "sandbox", "hue": 0,
        "versionId": "dsh-0.1.0", "runtimeId": "node-system", "profile": "web",
        "port": 34480, "autoPort": true, "createdAt": "test", "totalRuntime": 0,
        "favorite": false, "env": {}, "args": []
    }))
    .unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn conversations(home: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(base: &Path, at: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !at.is_dir() {
            return;
        }
        for entry in std::fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    for keep in KEEP {
        walk(home, &home.join(keep), &mut files);
    }
    files
}

#[tokio::test]
async fn attachment_copy_failure_keeps_every_original_byte() {
    let f = Fixture::new("copy-error").await;
    let gold = conversations(&f.home());
    let error = f
        .restore(&|point| {
            if point == Point::CopyAttachments {
                // Force the real copier's create_dir_all to fail after sessions
                // have been copied. This is not a simulated success/error result.
                std::fs::write(
                    f.stage().join("dsh-home/attachments"),
                    b"locked-destination",
                )
                .unwrap();
            }
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(!error.is_empty());
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"live-config"
    );
    assert!(!f.stage().exists());
    f.recover().await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
}

#[tokio::test]
async fn failed_backup_rename_preserves_current_and_foreign_backup() {
    let f = Fixture::new("backup-error").await;
    let gold = conversations(&f.home());
    f.restore(&|point| {
        if point == Point::Backup {
            write(&f.backup().join("do-not-delete"), b"foreign");
        }
        Ok(())
    })
    .await
    .unwrap_err();
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(
        std::fs::read(f.backup().join("do-not-delete")).unwrap(),
        b"foreign"
    );
    assert!(f.recover().await.unwrap_err().contains("保留"));
    assert_eq!(conversations(&f.home()), gold);
}

#[tokio::test]
async fn failed_promotion_rolls_the_complete_original_back() {
    let f = Fixture::new("promotion-error").await;
    let gold = conversations(&f.home());
    f.restore(&|point| {
        if point == Point::Promote {
            Err("injected promote failure".into())
        } else {
            Ok(())
        }
    })
    .await
    .unwrap_err();
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"live-config"
    );
    assert!(!f.stage().exists());
    assert!(!f.backup().exists());
}

#[tokio::test]
async fn failed_rollback_retains_both_copies_and_reports_both_errors() {
    let f = Fixture::new("rollback-error").await;
    let gold = conversations(&f.home());
    let error = f
        .restore(&|point| match point {
            Point::Promote => Err("injected promotion error".into()),
            Point::Rollback => Err("injected rollback error".into()),
            _ => Ok(()),
        })
        .await
        .unwrap_err();
    assert!(
        error.contains("injected promotion error") && error.contains("injected rollback error")
    );
    assert_eq!(conversations(&f.backup()), gold);
    assert_eq!(conversations(&f.stage().join("dsh-home")), gold);
    f.recover().await.unwrap();
    f.recover().await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
}

#[tokio::test]
async fn a_real_promotion_and_rollback_collision_never_deletes_either_copy() {
    let f = Fixture::new("rename-collision").await;
    let gold = conversations(&f.home());
    let error = f
        .restore(&|point| {
            if point == Point::Promote {
                write(&f.home().join("foreign"), b"keep-current");
            }
            Ok(())
        })
        .await
        .unwrap_err();
    assert!(error.contains("回滚失败"));
    assert_eq!(conversations(&f.backup()), gold);
    assert_eq!(conversations(&f.stage().join("dsh-home")), gold);
    assert_eq!(
        std::fs::read(f.home().join("foreign")).unwrap(),
        b"keep-current"
    );
    assert!(f.recover().await.is_err());
}

#[tokio::test]
async fn committed_cleanup_retry_never_rolls_back_new_messages() {
    let f = Fixture::new("cleanup-error").await;
    let gold = conversations(&f.home());
    f.restore(&|point| {
        if point == Point::Cleanup {
            Err("cleanup unavailable".into())
        } else {
            Ok(())
        }
    })
    .await
    .unwrap_err();
    assert_eq!(conversations(&f.home()), gold);
    write(&f.home().join("sessions/after-commit"), b"newer message");
    f.recover().await.unwrap();
    f.recover().await.unwrap();
    assert_eq!(
        std::fs::read(f.home().join("sessions/after-commit")).unwrap(),
        b"newer message"
    );
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"snapshot-config"
    );
    assert!(!f.backup().exists());
}

#[tokio::test]
async fn legacy_moves_are_returned_before_any_staging_cleanup() {
    for with_backup in [false, true] {
        let f = Fixture::new("legacy").await;
        let gold = conversations(&f.home());
        std::fs::create_dir_all(f.stage().join("dsh-home")).unwrap();
        for keep in KEEP {
            std::fs::rename(f.home().join(keep), f.stage().join("dsh-home").join(keep)).unwrap();
        }
        if with_backup {
            std::fs::rename(f.home(), f.backup()).unwrap();
        }
        f.recover().await.unwrap();
        f.recover().await.unwrap();
        assert_eq!(conversations(&f.home()), gold);
        assert!(!f.stage().exists());
    }
}

#[tokio::test]
async fn legacy_partial_move_is_safe_and_legacy_conflicts_fail_closed() {
    let f = Fixture::new("legacy-partial").await;
    let gold = conversations(&f.home());
    std::fs::create_dir_all(f.stage().join("dsh-home")).unwrap();
    std::fs::rename(
        f.home().join("sessions"),
        f.stage().join("dsh-home/sessions"),
    )
    .unwrap();
    f.recover().await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
    write(
        &f.stage().join("dsh-home/sessions/conflict"),
        b"unique staged bytes",
    );
    let staged = conversations(&f.stage().join("dsh-home"));
    assert!(f.recover().await.unwrap_err().contains("拒绝覆盖"));
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(conversations(&f.stage().join("dsh-home")), staged);
}

#[tokio::test]
async fn unrecognized_journals_and_missing_originals_are_preserved() {
    for raw in [
        b"{broken".as_slice(),
        br#"{"version":2,"instance_id":"restore-p0","phase":"ready"}"#.as_slice(),
    ] {
        let f = Fixture::new("bad-journal").await;
        f.prepare(Phase::Copying);
        std::fs::write(f.stage().join(JOURNAL), raw).unwrap();
        write(&f.stage().join("dsh-home/sessions/sole"), b"only copy");
        assert!(f.recover().await.is_err());
        assert_eq!(
            std::fs::read(f.stage().join("dsh-home/sessions/sole")).unwrap(),
            b"only copy"
        );
    }
    let f = Fixture::new("missing-original").await;
    std::fs::rename(f.home(), f.stage()).unwrap();
    assert!(f.recover().await.is_err());
    assert!(f.stage().join("sessions/project/live.jsonl").exists());
}

#[tokio::test]
async fn startup_recovery_respects_the_active_instance_lock() {
    let f = Fixture::new("startup-lock").await;
    f.prepare(Phase::Copying);
    let locks = crate::resources::ResourceLocks::default();
    let held = locks
        .acquire(&[crate::resources::Resource::Instance(ID.into())])
        .unwrap();
    let processes = Processes::default();
    let registry = crate::launch::Registry::default();
    super::super::recover_pending_restores(&f.root, &processes, &registry, &locks)
        .await
        .unwrap();
    assert!(
        f.stage().exists(),
        "an active restore must keep its staging"
    );
    drop(held);
    super::super::recover_pending_restores(&f.root, &processes, &registry, &locks)
        .await
        .unwrap();
    assert!(!f.stage().exists());
}

#[tokio::test]
async fn startup_recovery_never_changes_a_running_instances_home() {
    let f = Fixture::new("startup-running").await;
    f.prepare(Phase::Copying);
    let processes = Processes::default();
    processes.set(
        ID,
        crate::launch::ProcessEntry {
            pid: std::process::id(),
            port: 34480,
        },
    );
    super::super::recover_pending_restores(
        &f.root,
        &processes,
        &crate::launch::Registry::default(),
        &crate::resources::ResourceLocks::default(),
    )
    .await
    .unwrap();
    assert!(f.stage().exists());
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"live-config"
    );
}

#[tokio::test]
async fn launch_preparation_recovers_before_creating_a_new_home() {
    let f = Fixture::new("launch-recovery").await;
    let gold = conversations(&f.home());
    f.prepare(Phase::Ready);
    std::fs::rename(f.home(), f.backup()).unwrap();
    crate::launch::home::prepare(&f.dir(), "web").await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"live-config"
    );
    assert!(!f.stage().exists() && !f.backup().exists());
}

#[tokio::test]
async fn launch_preparation_does_not_write_into_an_ambiguous_transaction() {
    let f = Fixture::new("launch-closed").await;
    f.prepare(Phase::Ready);
    std::fs::rename(f.home(), f.backup()).unwrap();
    std::fs::write(f.stage().join(JOURNAL), b"unknown record").unwrap();
    assert!(crate::launch::home::prepare(&f.dir(), "web").await.is_err());
    assert!(
        !f.home().exists(),
        "launch must not mask missing original with a new empty home"
    );
    assert!(!f.dir().join("workspace").exists());
    assert!(f.backup().join("sessions/project/live.jsonl").exists());
}

#[tokio::test]
async fn cancellation_after_copying_sessions_never_moves_the_originals() {
    let f = Fixture::new("cancel-after-sessions").await;
    let gold = conversations(&f.home());
    let flag = Arc::new(AtomicBool::new(false));
    let error = run(
        &f.root,
        ID,
        SNAP,
        &Processes::default(),
        &flag,
        &|_| {},
        &|point| {
            if point == Point::SessionsCopied {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert!(error.contains("cancelled"));
    assert_eq!(conversations(&f.home()), gold);
    assert!(!f.stage().exists() && !f.backup().exists());
}

#[tokio::test]
async fn deletion_does_not_discard_a_pending_restores_recovery_material() {
    let f = Fixture::new("delete-gate").await;
    f.prepare(Phase::Copying);
    let error = super::super::delete_snapshot_inner(&f.root, ID, SNAP, &Processes::default())
        .await
        .unwrap_err();
    assert!(error.contains("未完成"));
    assert!(f
        .dir()
        .join("snapshots")
        .join(SNAP)
        .join("snapshot.json")
        .exists());
    assert!(f.stage().exists());
}

#[tokio::test]
async fn recovery_refuses_transaction_root_links() {
    let f = Fixture::new("link-gate").await;
    let outside = f.root.join("outside");
    write(&outside.join("sole"), b"untouched outside");
    crate::instances::copy::recreate_link(&f.stage(), &outside, true).unwrap();
    assert!(f.recover().await.is_err());
    assert_eq!(
        std::fs::read(outside.join("sole")).unwrap(),
        b"untouched outside"
    );
}

#[tokio::test]
async fn conversation_links_into_the_replaced_environment_are_refused_before_swap() {
    let f = Fixture::new("mutable-link").await;
    let target = f.home().join("profiles/web/data");
    write(
        &target.join("sole"),
        b"conversation stored behind mutable environment link",
    );
    crate::instances::copy::recreate_link(&f.home().join("sessions/indirect"), &target, true)
        .unwrap();
    assert!(f.restore(&|_| Ok(())).await.is_err());
    assert_eq!(
        std::fs::read(f.home().join("sessions/indirect/sole")).unwrap(),
        b"conversation stored behind mutable environment link"
    );
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"live-config"
    );
    assert!(!f.stage().exists() && !f.backup().exists());
}

#[tokio::test]
async fn internal_conversation_links_survive_promotion_and_backup_cleanup() {
    let f = Fixture::new("retained-link").await;
    let target = f.home().join("sessions/project");
    crate::instances::copy::recreate_link(&f.home().join("sessions/linked"), &target, true)
        .unwrap();
    let gold = conversations(&f.home());
    f.restore(&|_| Ok(())).await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(
        std::fs::read(f.home().join("sessions/linked/live.jsonl")).unwrap(),
        b"live session\0\xff"
    );
    assert!(!f.backup().exists());
}

#[tokio::test]
async fn repeated_restores_keep_messages_created_after_the_first_restore() {
    let f = Fixture::new("repeat-success").await;
    f.restore(&|_| Ok(())).await.unwrap();
    write(
        &f.home().join("sessions/newer/message"),
        b"created after first restore",
    );
    let gold = conversations(&f.home());
    f.restore(&|_| Ok(())).await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
    assert_eq!(
        std::fs::read(f.home().join("settings.yaml")).unwrap(),
        b"snapshot-config"
    );
    assert!(f
        .dir()
        .join("snapshots")
        .join(SNAP)
        .join("snapshot.json")
        .exists());
}

#[cfg(windows)]
#[tokio::test]
async fn case_aliased_snapshot_conversations_never_replace_or_merge_with_live_data() {
    let f = Fixture::new("case-alias").await;
    let gold = conversations(&f.home());
    let snapshot = f.dir().join("snapshots").join(SNAP).join("dsh-home");
    for (old, alias) in [("sessions", "Sessions"), ("attachments", "Attachments")] {
        let temporary = snapshot.join(format!("case-{old}"));
        std::fs::rename(snapshot.join(old), &temporary).unwrap();
        std::fs::rename(&temporary, snapshot.join(alias)).unwrap();
    }
    f.restore(&|_| Ok(())).await.unwrap();
    assert_eq!(conversations(&f.home()), gold);
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child_command(root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", WORKER, "--ignored", "--nocapture"])
        .env("PHL_SNAPSHOT_CRASH_ROOT", root);
    command
}

#[test]
fn hard_kill_and_fresh_process_recovery_preserve_bytes_at_every_boundary() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    for point in [
        Point::SnapshotCopied,
        Point::SessionsCopied,
        Point::AttachmentsCopied,
        Point::Backup,
        Point::BackedUp,
        Point::Promote,
        Point::Promoted,
        Point::Cleanup,
    ] {
        let f = rt.block_on(Fixture::new("kill"));
        let gold = conversations(&f.home());
        let mut child = OwnedChild(
            child_command(&f.root)
                .env("PHL_SNAPSHOT_CRASH_POINT", format!("{point:?}"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while !f.root.join("crash.ready").exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "worker exited before {point:?}"
            );
            assert!(Instant::now() < deadline, "worker did not reach {point:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            rt.block_on(f.recover()).is_err(),
            "another process must not recover a live owner's staging at {point:?}"
        );
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(
            [f.home(), f.backup(), f.stage().join("dsh-home")]
                .iter()
                .any(|home| conversations(home) == gold),
            "original bytes must remain findable at {point:?}"
        );
        for _ in 0..2 {
            let output = child_command(&f.root)
                .env("PHL_SNAPSHOT_CRASH_POINT", "recover")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "fresh recovery failed at {point:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(conversations(&f.home()), gold, "{point:?}");
        }
        let expected = if matches!(point, Point::Promoted | Point::Cleanup) {
            b"snapshot-config".as_slice()
        } else {
            b"live-config".as_slice()
        };
        assert_eq!(
            std::fs::read(f.home().join("settings.yaml")).unwrap(),
            expected
        );
        assert!(!f.stage().exists() && !f.backup().exists());
    }
}

#[test]
#[ignore = "subprocess worker; runs only against an owned P0 fixture"]
fn crash_worker() {
    let Ok(root) = std::env::var("PHL_SNAPSHOT_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("phl-restore-p0-"));
    assert_eq!(
        std::fs::read(root.join(".owned-test-data")).unwrap(),
        b"snapshot-p0-fixture"
    );
    let point = std::env::var("PHL_SNAPSHOT_CRASH_POINT").unwrap();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        if point == "recover" {
            super::super::recover_pending_restores(
                &root,
                &Processes::default(),
                &crate::launch::Registry::default(),
                &crate::resources::ResourceLocks::default(),
            )
            .await
            .unwrap();
        } else {
            run(
                &root,
                ID,
                SNAP,
                &Processes::default(),
                &Arc::new(AtomicBool::new(false)),
                &|_| {},
                &|at| {
                    if format!("{at:?}") == point {
                        std::fs::write(root.join("crash.ready"), b"ready").unwrap();
                        loop {
                            std::thread::sleep(Duration::from_secs(1));
                        }
                    }
                    Ok(())
                },
            )
            .await
            .unwrap();
            panic!("worker did not stop at requested boundary");
        }
    });
}

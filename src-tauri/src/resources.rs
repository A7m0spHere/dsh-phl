//! Resource-level operation mutex and task registry.
//!
//! Transfers (`versions::Transfers`) track *cancellation* per attempt id;
//! they say nothing about whether two operations may run at the same time.
//! That was the frontend's job — the store's `pluginInstallActive` flag
//! blocked plugin installs against snapshots only as long as the WebView was
//! the sole caller, and every crash between "check" and "write" raced.
//!
//! Here the backend is authoritative: a mutating command atomically acquires
//! the resources it will rewrite before touching the disk, and the guard
//! releases on drop — including on early `?` returns and panics, so no error
//! path leaks a held lock. The conflict rules:
//!
//! - the same instance, version or runtime is exclusive with itself;
//! - a data-root migration is exclusive with everything (it moves all three);
//! - unrelated resources run concurrently (different instances never block
//!   each other).
//!
//! Task ids and resource keys are deliberately separate concepts: one task
//! can hold several resources, and the same resource is serially claimed by
//! many tasks over its lifetime. The registry keeps the recent history the
//! frontend's task centre renders: kind, label, resources, requested phase,
//! cancel request, outcome and error.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::State;

/// A unit of on-disk state that exactly one mutating operation may rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resource {
    Instance(String),
    Version(String),
    Runtime(String),
    /// The whole data root — held by `move_root_data` only.
    DataRoot,
}

/// The data-root key. Its special status is defined once, in `conflicts`.
const ROOT_KEY: &str = "root";

impl Resource {
    pub fn key(&self) -> String {
        match self {
            Resource::Instance(id) => format!("instance:{id}"),
            Resource::Version(v) => format!("version:{v}"),
            Resource::Runtime(r) => format!("runtime:{r}"),
            Resource::DataRoot => ROOT_KEY.to_string(),
        }
    }
}

fn conflicts(a: &str, b: &str) -> bool {
    a == b || a == ROOT_KEY || b == ROOT_KEY
}

/// Turns a held key into "实例 main" / "版本 0.17.3" style wording.
fn describe(key: &str) -> String {
    match key.split_once(':') {
        Some(("instance", name)) => format!("实例 {name}"),
        Some(("version", name)) => format!("版本 {name}"),
        Some(("runtime", name)) => format!("Runtime {name}"),
        _ => "数据目录".to_string(),
    }
}

/// What was busy when an acquisition failed, so the UI can name the wait.
#[derive(Debug)]
pub struct Conflict {
    /// The resource key we tried to acquire.
    pub resource: String,
    /// The key currently held by another operation.
    pub held_by: String,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.held_by == ROOT_KEY {
            write!(
                f,
                "{} 正被数据目录迁移占用，迁移完成后才能操作",
                describe(&self.resource)
            )
        } else {
            write!(
                f,
                "{} 正被另一个操作占用，请等待其完成后重试",
                describe(&self.resource)
            )
        }
    }
}

/// Releases the acquired keys when dropped.
#[derive(Debug)]
pub struct Held {
    // Debug for test assertions on `Result::unwrap`.
    shared: Arc<Mutex<HashSet<String>>>,
    keys: Vec<String>,
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut held = self.shared.lock().expect("resource locks");
        for key in &self.keys {
            held.remove(key);
        }
    }
}

#[derive(Clone, Default)]
pub struct ResourceLocks {
    held: Arc<Mutex<HashSet<String>>>,
}

impl ResourceLocks {
    /// Atomically take every resource or none. The check and the insert share
    /// one critical section, so two racing acquires cannot both pass it.
    pub fn acquire(&self, resources: &[Resource]) -> Result<Held, Conflict> {
        let mut held = self.held.lock().expect("resource locks");
        let keys: Vec<String> = resources.iter().map(|r| r.key()).collect();
        for key in &keys {
            if let Some(busy) = held.iter().find(|h| conflicts(key, h)) {
                return Err(Conflict {
                    resource: key.clone(),
                    held_by: busy.clone(),
                });
            }
        }
        held.extend(keys.iter().cloned());
        Ok(Held {
            shared: self.held.clone(),
            keys,
        })
    }

    /// Keys currently held — diagnostics only, never used for check-then-act.
    /// Test view of the held keys. Production code must never enumerate the
    /// lock set for display: conflicts surface through the coded Busy error
    /// (with its `held_by` wording), and task rows carry their own resources.
    #[cfg(test)]
    pub fn snapshot(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .held
            .lock()
            .expect("resource locks")
            .iter()
            .cloned()
            .collect();
        keys.sort();
        keys
    }
}

/* ------------------------------ task registry ------------------------------ */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Done,
    Failed,
    Cancelled,
}

/// One entry of the task registry. `id` is the frontend transfer/attempt id
/// when the operation has one (so cancel routes through the existing
/// mechanism); otherwise it is generated by [`next_task_id`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub id: String,
    pub kind: &'static str,
    pub label: String,
    pub resources: Vec<String>,
    pub phase: String,
    /// A real 0..=1 ratio for the current phase, or `None` when the phase is
    /// indeterminate (verification, npm/pnpm dependency installs). The task
    /// center must not render a bar for `None` — inventing a percentage is
    /// exactly the fake progress this field exists to eliminate.
    pub progress: Option<f64>,
    pub state: TaskState,
    pub cancel_requested: bool,
    pub error: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

impl TaskInfo {
    /// A fresh `Running` row; the caller passes it to [`Tasks::begin`].
    pub fn new(id: String, kind: &'static str, label: String, resources: &[Resource]) -> Self {
        TaskInfo {
            id,
            kind,
            label,
            resources: resources.iter().map(|r| r.key()).collect(),
            phase: "started".into(),
            progress: None,
            state: TaskState::Running,
            cancel_requested: false,
            error: None,
            started_at: now_ms(),
            finished_at: None,
        }
    }
}

const HISTORY_LIMIT: usize = 50;

#[derive(Clone, Default)]
pub struct Tasks {
    inner: Arc<Mutex<HashMap<String, TaskInfo>>>,
}

impl Tasks {
    /// Registers a running task and hands back a handle. A duplicate live id
    /// is refused rather than silently sharing one history row: the frontend
    /// is required to mint fresh attempt ids (see `Transfers`' doc comment).
    pub fn begin(&self, mut info: TaskInfo, flag: Option<Arc<AtomicBool>>) -> Result<Task, String> {
        // Seed the row with whatever cancellation already arrived: a user
        // abort can beat the command's own registration.
        info.cancel_requested = flag
            .as_ref()
            .map(|f| f.load(Ordering::SeqCst))
            .unwrap_or(false);
        let mut map = self.inner.lock().expect("tasks");
        if map
            .get(&info.id)
            .is_some_and(|t| t.state == TaskState::Running)
        {
            return Err(format!("任务 {} 正在进行中", info.id));
        }
        let id = info.id.clone();
        map.insert(id.clone(), info);
        trim_history(&mut map);
        Ok(Task {
            id,
            inner: self.inner.clone(),
            flag,
        })
    }

    pub fn request_cancel(&self, id: &str) {
        if let Some(info) = self.inner.lock().expect("tasks").get_mut(id) {
            info.cancel_requested = true;
        }
    }

    /// Snapshot of all recorded tasks, newest first.
    pub fn list(&self) -> Vec<TaskInfo> {
        let mut all: Vec<TaskInfo> = self
            .inner
            .lock()
            .expect("tasks")
            .values()
            .cloned()
            .collect();
        all.sort_by_key(|t| -t.started_at);
        all
    }
}

/// Clonable handle on one registry row; all clones update the same entry.
/// A clone dropped while the row is still `Running` records it as failed —
/// an operation that vanished mid-flight (panic, process exit) is never left
/// displayed as still running. [`guarded`] deliberately calls `finish` after
/// the body's clone drops, so a completed task's real outcome always wins
/// over the transient abandon marker.
#[derive(Clone)]
pub struct Task {
    id: String,
    inner: Arc<Mutex<HashMap<String, TaskInfo>>>,
    flag: Option<Arc<AtomicBool>>,
}

impl Task {
    /// Live read of the shared cancel flag — the same `AtomicBool` instance
    /// `Transfers::cancel` flips, so the registry and the cancel plumbing
    /// cannot disagree about whether an abort was requested.
    pub fn cancel_observed(&self) -> bool {
        let flag = self
            .flag
            .as_ref()
            .map(|f| f.load(Ordering::SeqCst))
            .unwrap_or(false);
        flag || self
            .inner
            .lock()
            .expect("tasks")
            .get(&self.id)
            .is_some_and(|info| info.cancel_requested)
    }

    /// Id only for log lines — the row itself lives in the registry.
    pub fn id_for_log(&self) -> &str {
        &self.id
    }

    pub fn set_phase(&self, phase: &str) {
        if let Some(info) = self.inner.lock().expect("tasks").get_mut(&self.id) {
            info.phase = phase.to_string();
        }
    }

    /// Pairs with [`set_phase`](Self::set_phase): `Some(0.0..=1.0)` for a
    /// phase with a real ratio, `None` to mark the phase indeterminate so the
    /// UI drops the bar instead of showing a stale or fabricated number.
    pub fn set_progress(&self, progress: Option<f64>) {
        if let Some(info) = self.inner.lock().expect("tasks").get_mut(&self.id) {
            info.progress = progress;
        }
    }

    /// Records the outcome: Done, or Cancelled when the user aborted (through
    /// either the flag or an explicit registry request), or Failed with the
    /// error message attached.
    pub fn finish(&self, outcome: Result<(), String>) {
        // Read the external flag before locking the task map. Calling
        // `cancel_observed()` while holding `inner` would try to lock the same
        // non-reentrant mutex again now that that method also observes registry
        // cancellation requests.
        let flag_cancelled = self
            .flag
            .as_ref()
            .map(|flag| flag.load(Ordering::SeqCst))
            .unwrap_or(false);
        if let Some(info) = self.inner.lock().expect("tasks").get_mut(&self.id) {
            let cancelled = flag_cancelled
                || info.cancel_requested
                || outcome.as_ref().err().is_some_and(|e| is_cancel_msg(e));
            // Every branch assigns `error` explicitly: a body clone dropping
            // at the end of the call may already have written the transient
            // "abandoned" marker (see the Drop impl), and a real outcome must
            // not inherit it.
            info.state = match outcome {
                Ok(()) if cancelled => {
                    info.error = None;
                    TaskState::Cancelled
                }
                Ok(()) => {
                    info.error = None;
                    TaskState::Done
                }
                Err(_) if cancelled => {
                    info.error = None;
                    TaskState::Cancelled
                }
                Err(e) => {
                    info.error = Some(e);
                    TaskState::Failed
                }
            };
            info.finished_at = Some(now_ms());
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if let Some(info) = self.inner.lock().expect("tasks").get_mut(&self.id) {
            if info.state == TaskState::Running {
                info.state = TaskState::Failed;
                info.error = Some("任务在执行中被中断（进程退出或 panic）".into());
                info.finished_at = Some(now_ms());
            }
        }
    }
}

/// Ids for operations that never had a frontend transfer id.
pub fn next_task_id(kind: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("task-{kind}-{n}")
}

/* ------------------------------ unified entry ------------------------------ */

/// The whole point of this module in one call: atomically acquire the
/// resources (or refuse with a `[busy]` conflict message and register
/// nothing — declined attempts never touched a resource, and recording every
/// refused click would bury the history under rows that did no work), then
/// register the task, run the body, record the outcome, release the locks —
/// on every path. The body receives a task handle for phase reporting
/// (`set_phase`, `cancel_observed`); `guarded` finishes the row with the
/// body's result.
#[allow(clippy::too_many_arguments)]
pub async fn guarded<T, Fut>(
    id: String,
    kind: &'static str,
    label: impl Into<String>,
    resources: Vec<Resource>,
    flag: Option<Arc<AtomicBool>>,
    locks: &ResourceLocks,
    tasks: &Tasks,
    f: impl FnOnce(Task) -> Fut,
) -> Result<T, String>
where
    Fut: Future<Output = Result<T, String>>,
{
    let _held = locks
        .acquire(&resources)
        .map_err(|c| crate::errors::coded(crate::errors::ErrCode::Busy, c))?;
    let info = TaskInfo::new(id, kind, label.into(), &resources);
    let task = tasks.begin(info, flag)?;
    // Correlation line: every later diagnostic that mentions the instance or
    // transfer id can be tied back to this task by grepping the id (O-09).
    eprintln!("[phl][task {}] {kind} 开始", task.id_for_log());
    let outcome = f(task.clone()).await;
    task.finish(outcome.as_ref().map(|_| ()).map_err(|e| e.clone()));
    match &outcome {
        Ok(_) => eprintln!("[phl][task {}] {kind} 完成", task.id_for_log()),
        Err(e) => eprintln!("[phl][task {}] {kind} 失败: {e}", task.id_for_log()),
    }
    // `_held` releases here, after the outcome is recorded: a waiter seeing
    // the task finish never races a still-locked resource.
    drop(_held);
    outcome
}

fn is_cancel_msg(err: &str) -> bool {
    err == "cancelled"
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn trim_history(map: &mut HashMap<String, TaskInfo>) {
    let finished: Vec<String> = {
        let mut rows: Vec<(i64, String)> = map
            .iter()
            .filter(|(_, t)| t.state != TaskState::Running)
            .map(|(id, t)| (t.started_at, id.clone()))
            .collect();
        if rows.len() <= HISTORY_LIMIT {
            return;
        }
        rows.sort();
        let excess = rows.len() - HISTORY_LIMIT;
        rows.into_iter().take(excess).map(|(_, id)| id).collect()
    };
    for id in finished {
        map.remove(&id);
    }
}

/* ------------------------------ commands ------------------------------ */

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    pub tasks: Vec<TaskInfo>,
}

/// Recent and in-flight long tasks, for the task centre.
///
/// This used to also ship a raw `held` key list, polled and stored by the
/// frontend but consumed nowhere: a task row already carries the `resources`
/// it holds, and a *conflict* is explained through the coded Busy error
/// (whose `held_by` renders the owning operation) — the snapshot added a
/// second, un-labelled channel for the same facts and was deleted per audit.
#[tauri::command]
pub fn list_tasks(tasks: State<'_, Tasks>) -> TaskList {
    TaskList {
        tasks: tasks.list(),
    }
}

/* ------------------------------ tests ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    fn instance(id: &str) -> Resource {
        Resource::Instance(id.to_string())
    }

    #[test]
    fn same_resource_is_exclusive_and_releases_after_the_guard_drops() {
        let locks = ResourceLocks::default();
        let _held = locks.acquire(&[instance("main")]).unwrap();
        let err = locks.acquire(&[instance("main")]).unwrap_err();
        assert!(err.to_string().contains("实例 main"), "{err}");
        drop(_held);
        assert!(locks.acquire(&[instance("main")]).is_ok());
    }

    #[test]
    fn unrelated_resources_never_block_each_other() {
        let locks = ResourceLocks::default();
        let _a = locks.acquire(&[instance("main")]).unwrap();
        let _b = locks.acquire(&[instance("other")]).unwrap();
        let _c = locks
            .acquire(&[Resource::Version("0.17.3".into())])
            .unwrap();
        let _d = locks
            .acquire(&[Resource::Runtime("node-22".into())])
            .unwrap();
    }

    #[test]
    fn migration_is_exclusive_with_everything_both_ways() {
        let locks = ResourceLocks::default();
        let root = locks.acquire(&[Resource::DataRoot]).unwrap();
        assert!(locks.acquire(&[instance("main")]).is_err());
        assert!(locks.acquire(&[Resource::Version("1".into())]).is_err());
        drop(root);
        let inst = locks.acquire(&[instance("main")]).unwrap();
        assert!(locks.acquire(&[Resource::DataRoot]).is_err());
        drop(inst);
        assert!(locks.acquire(&[Resource::DataRoot]).is_ok());
    }

    #[test]
    fn multi_resource_acquire_is_all_or_nothing() {
        let locks = ResourceLocks::default();
        let _busy = locks.acquire(&[instance("dst")]).unwrap();
        let err = locks
            .acquire(&[instance("src"), instance("dst")])
            .expect_err("the busy side must refuse the whole pair");
        assert!(err.resource.contains("dst"));
        // The free side was never taken: after the busy side releases, a
        // single acquire of it succeeds (no leftover from the failed pair).
        drop(_busy);
        assert!(locks.acquire(&[instance("src")]).is_ok());
    }

    #[tokio::test]
    async fn guarded_records_success_phase_and_releases_resources() {
        let (locks, tasks) = (ResourceLocks::default(), Tasks::default());
        let inside = locks.clone(); // same underlying map, ownable by the body
        guarded(
            "t1".into(),
            "plugin-install",
            "安装 dsh-foo",
            vec![instance("main")],
            None,
            &locks,
            &tasks,
            move |task| async move {
                task.set_phase("committing");
                assert!(
                    inside.acquire(&[instance("main")]).is_err(),
                    "locked during the body"
                );
                Ok::<(), String>(())
            },
        )
        .await
        .unwrap();
        let rows = tasks.list();
        assert_eq!(rows[0].state, TaskState::Done);
        assert_eq!(rows[0].phase, "committing");
        assert!(locks.acquire(&[instance("main")]).is_ok());
    }

    #[tokio::test]
    async fn guarded_error_is_a_failed_task_and_still_releases() {
        let (locks, tasks) = (ResourceLocks::default(), Tasks::default());
        let err = guarded::<(), _>(
            "t2".into(),
            "snapshot",
            "快照",
            vec![instance("main")],
            None,
            &locks,
            &tasks,
            |_| async { Err("磁盘满了".to_string()) },
        )
        .await
        .unwrap_err();
        assert_eq!(err, "磁盘满了");
        let rows = tasks.list();
        assert_eq!(rows[0].state, TaskState::Failed);
        assert_eq!(rows[0].error.as_deref(), Some("磁盘满了"));
        assert!(locks.snapshot().is_empty());
    }

    #[tokio::test]
    async fn cancel_via_the_shared_flag_lands_as_cancelled() {
        let (locks, tasks) = (ResourceLocks::default(), Tasks::default());
        let flag = Arc::new(AtomicBool::new(false));
        let seen = {
            let flag = flag.clone();
            guarded::<(), _>(
                "t3".into(),
                "version-install",
                "下载版本",
                vec![Resource::Version("1".into())],
                Some(flag.clone()),
                &locks,
                &tasks,
                move |_| async move {
                    flag.store(true, Ordering::SeqCst);
                    Err("cancelled".to_string())
                },
            )
            .await
        };
        assert!(seen.is_err());
        assert_eq!(tasks.list()[0].state, TaskState::Cancelled);
    }

    #[tokio::test]
    async fn cancel_beating_registration_still_cancels() {
        // The frontend aborts before `guarded` registers the row — the flag
        // is already true when begin() runs, and the Done/Failed decision
        // must still read `cancelled`.
        let (locks, tasks) = (ResourceLocks::default(), Tasks::default());
        let flag = Arc::new(AtomicBool::new(true));
        let out = guarded::<(), _>(
            "t4".into(),
            "runtime-install",
            "安装 runtime",
            vec![Resource::Runtime("node-22".into())],
            Some(flag),
            &locks,
            &tasks,
            |task| async move {
                if task.cancel_observed() {
                    return Err("cancelled".into());
                }
                Ok(())
            },
        )
        .await;
        assert!(out.is_err());
        assert_eq!(tasks.list()[0].state, TaskState::Cancelled);
    }

    #[test]
    fn a_panic_is_not_left_running() {
        let (locks, tasks) = (ResourceLocks::default(), Tasks::default());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            rt.block_on(guarded::<(), _>(
                "t5".into(),
                "plugin-install",
                "炸了",
                vec![instance("main")],
                None,
                &locks,
                &tasks,
                |_| async { panic!("boom") },
            ))
        }));
        assert!(result.is_err());
        let rows = tasks.list();
        assert_eq!(rows[0].state, TaskState::Failed);
        assert!(rows[0].error.as_deref().unwrap().contains("中断"));
        // The lock guard unwound too.
        assert!(locks.snapshot().is_empty());
    }

    #[tokio::test]
    async fn duplicate_live_id_is_refused_not_shared() {
        let (locks, tasks) = (ResourceLocks::default(), Tasks::default());
        // An operation already registered under this id keeps the row live.
        let live = tasks
            .begin(
                TaskInfo::new("dup".into(), "plugin-install", "第一个".into(), &[]),
                None,
            )
            .unwrap();
        let err = guarded::<(), _>(
            "dup".into(),
            "plugin-install",
            "第二个",
            vec![instance("b")],
            None,
            &locks,
            &tasks,
            |_| async { Ok(()) },
        )
        .await
        .unwrap_err();
        assert!(err.contains("正在进行中"), "{err}");
        live.finish(Ok(()));
    }

    #[test]
    fn history_is_capped() {
        let tasks = Tasks::default();
        let ids: Vec<String> = (0..(HISTORY_LIMIT + 25))
            .map(|i| format!("old-{i}"))
            .collect();
        for (i, id) in ids.into_iter().enumerate() {
            let mut info = TaskInfo::new(id, "test", String::new(), &[]);
            // Distinct increasing timestamps so "newest survived" is a real
            // assertion, not a tie inside one wall-clock millisecond.
            info.started_at = i as i64;
            let t = tasks.begin(info, None).unwrap();
            t.finish(Ok(()));
        }
        // Trimming happens when a task begins, so the newest finished row
        // survives until one more task arrives — stale history is capped at
        // HISTORY_LIMIT, the cap is not an exact-size invariant.
        assert_eq!(tasks.list().len(), HISTORY_LIMIT + 1);
        // Newest survived, oldest pruned.
        assert_eq!(tasks.list()[0].id, format!("old-{}", HISTORY_LIMIT + 24));
        let _newer = tasks
            .begin(
                TaskInfo::new("newer".into(), "test", String::new(), &[]),
                None,
            )
            .unwrap();
        assert_eq!(tasks.list().len(), HISTORY_LIMIT + 1); // 50 stale + 1 running + ...
        assert_eq!(
            tasks
                .list()
                .iter()
                .filter(|t| t.state == TaskState::Running)
                .count(),
            1
        );
    }
}

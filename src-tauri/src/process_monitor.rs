//! Read-only sampling of PHL's registered DSH root processes. No caller-supplied
//! PIDs, process arguments or environment are exposed by this command.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::State;

use crate::launch::{decide, probe_process, Adoption, PersistedProcess, Processes, Registry};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSample {
    pub instance_id: String,
    pub pid: u32,
    pub port: u16,
    /// running | exited | unknown. Unknown never means zero resource usage.
    pub status: String,
    pub started_at_ms: Option<i64>,
    pub sampled_at_ms: u64,
    /// Percentage of all logical CPUs available to PHL (0..100).
    pub cpu_percent: Option<f32>,
    /// Resident bytes of the DSH root process, including shared resident pages.
    pub memory_bytes: Option<u64>,
    pub reason: Option<String>,
}

struct TrackedProcess {
    instance_id: String,
    pid: u32,
    port: u16,
    record: Option<PersistedProcess>,
}

#[derive(Default)]
struct Sampler {
    system: System,
    topology: Vec<(String, u32)>,
    previous: HashMap<String, (u32, Option<String>, u64)>,
    last_refresh: Option<Instant>,
    samples: Vec<ProcessSample>,
}

#[derive(Default)]
pub struct Monitor(Arc<Mutex<Sampler>>);

#[tauri::command]
pub async fn sample_instance_processes(
    monitor: State<'_, Monitor>,
    processes: State<'_, Processes>,
    registry: State<'_, Registry>,
) -> Result<Vec<ProcessSample>, String> {
    let entries: Vec<_> = processes
        .0
        .lock()
        .map_err(|_| "进程登记不可用")?
        .iter()
        .map(|(id, entry)| (id.clone(), *entry))
        .collect();
    let tracked = entries
        .into_iter()
        .map(|(instance_id, entry)| TrackedProcess {
            record: registry.record_of(&instance_id),
            instance_id,
            pid: entry.pid,
            port: entry.port,
        })
        .collect();
    let sampler = Arc::clone(&monitor.0);
    tauri::async_runtime::spawn_blocking(move || {
        sampler
            .lock()
            .map_err(|_| "进程采样器不可用".to_string())
            .map(|mut sampler| sampler.sample(tracked))
    })
    .await
    .map_err(|error| format!("进程采样失败: {error}"))?
}

impl Sampler {
    fn sample(&mut self, mut tracked: Vec<TrackedProcess>) -> Vec<ProcessSample> {
        tracked.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
        let topology: Vec<_> = tracked
            .iter()
            .map(|entry| (entry.instance_id.clone(), entry.pid))
            .collect();
        if self.topology != topology {
            // Keep the native cache bounded to current registrations. Reset CPU
            // baselines when membership changes, including stop/restart cycles.
            self.system = System::new();
            self.previous.clear();
            self.last_refresh = None;
            self.topology = topology;
        }
        if self
            .last_refresh
            .is_some_and(|at| at.elapsed() < sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
        {
            return self.samples.clone();
        }
        let pids: Vec<_> = tracked
            .iter()
            .map(|entry| Pid::from_u32(entry.pid))
            .collect();
        if !pids.is_empty() {
            self.system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&pids),
                true,
                ProcessRefreshKind::nothing()
                    .with_cpu()
                    .with_memory()
                    .without_tasks(),
            );
        }
        let sampled_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let logical_cpus = std::thread::available_parallelism()
            .ok()
            .map(|count| count.get());
        self.samples = tracked
            .iter()
            .map(|entry| {
                let mut sample = ProcessSample {
                    instance_id: entry.instance_id.clone(),
                    pid: entry.pid,
                    port: entry.port,
                    status: "unknown".into(),
                    started_at_ms: None,
                    sampled_at_ms,
                    cpu_percent: None,
                    memory_bytes: None,
                    reason: None,
                };
                let observed = probe_process(entry.pid);
                if observed.state == crate::launch::ProcessState::Exited {
                    sample.status = "exited".into();
                    sample.reason = Some("进程已退出，等待实例状态同步".into());
                    self.previous.remove(&entry.instance_id);
                    return sample;
                }
                let Some(record) = &entry.record else {
                    sample.reason = Some("缺少进程身份登记".into());
                    return sample;
                };
                #[cfg(any(windows, target_os = "macos"))]
                if record.process_start_token.is_none()
                    || record.process_start_token != observed.process_start_token
                {
                    sample.reason =
                        Some("进程创建标识缺失或不匹配，请查看日志并核对实例状态".into());
                    self.previous.remove(&entry.instance_id);
                    return sample;
                }
                if let Adoption::Forget { reason, .. } = decide(record, &observed) {
                    sample.reason = Some(reason);
                    self.previous.remove(&entry.instance_id);
                    return sample;
                }
                let Some(process) = self.system.process(Pid::from_u32(entry.pid)) else {
                    sample.reason = Some("系统未返回进程资源数据".into());
                    return sample;
                };
                // Check again after the native refresh. A recycled PID may not
                // donate the resource usage of another process to this instance.
                let after = probe_process(entry.pid);
                if !matches!(decide(record, &after), Adoption::Adopt)
                    || observed.exe_path != after.exe_path
                    || observed.process_start_token != after.process_start_token
                    || process
                        .start_time()
                        .abs_diff(observed.created_at_ms.unwrap_or(0).max(0) as u64 / 1000)
                        > 2
                {
                    sample.reason = Some("采样期间进程身份发生变化".into());
                    self.previous.remove(&entry.instance_id);
                    return sample;
                }
                let identity = (
                    entry.pid,
                    observed.process_start_token,
                    process.start_time(),
                );
                let warmed = self.previous.get(&entry.instance_id) == Some(&identity);
                self.previous.insert(entry.instance_id.clone(), identity);
                sample.status = "running".into();
                sample.started_at_ms = Some(process.start_time().saturating_mul(1000) as i64);
                sample.memory_bytes = (process.memory() > 0).then_some(process.memory());
                sample.cpu_percent = logical_cpus.and_then(|count| {
                    let percent = process.cpu_usage() / count as f32;
                    (warmed && percent.is_finite()).then_some(percent.clamp(0.0, 100.0))
                });
                sample
            })
            .collect();
        self.last_refresh = Some(Instant::now());
        self.samples.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn this_process() -> TrackedProcess {
        let pid = std::process::id();
        let probe = probe_process(pid);
        TrackedProcess {
            instance_id: "probe".into(),
            pid,
            port: 3080,
            record: Some(PersistedProcess {
                instance_id: "probe".into(),
                pid,
                port: 3080,
                started_at_ms: probe.created_at_ms.unwrap(),
                exe_path: probe.exe_path.unwrap(),
                process_start_token: probe.process_start_token,
            }),
        }
    }

    #[test]
    fn process_monitor_reads_real_memory_and_warms_cpu_before_reporting_it() {
        let mut sampler = Sampler::default();
        let first = sampler.sample(vec![this_process()]);
        assert_eq!(first[0].status, "running", "{first:?}");
        assert!(first[0].memory_bytes.unwrap() > 0);
        assert!(first[0].cpu_percent.is_none());
        std::thread::sleep(
            sysinfo::MINIMUM_CPU_UPDATE_INTERVAL + std::time::Duration::from_millis(100),
        );
        let next = sampler.sample(vec![this_process()]);
        assert!(next[0].cpu_percent.is_some());
        assert!(next[0].started_at_ms.unwrap() > 0);
        assert!(sampler.sample(vec![]).is_empty());
        assert!(sampler.previous.is_empty());
    }

    #[test]
    fn process_monitor_refuses_a_reused_pid_or_missing_identity() {
        let mut entry = this_process();
        entry.record.as_mut().unwrap().exe_path = "different-program".into();
        let mut sampler = Sampler::default();
        let result = sampler.sample(vec![entry]);
        assert_eq!(result[0].status, "unknown");
        assert!(result[0].memory_bytes.is_none());
        let mut entry = this_process();
        entry.record = None;
        sampler.last_refresh = None;
        let result = sampler.sample(vec![entry]);
        assert_eq!(result[0].status, "unknown");
        assert!(result[0].cpu_percent.is_none());
    }

    #[test]
    #[cfg(any(windows, target_os = "macos"))]
    fn process_monitor_rejects_same_executable_pid_reuse_and_legacy_records() {
        let mut entry = this_process();
        entry.record.as_mut().unwrap().process_start_token = Some("previous-process".into());
        let mut sampler = Sampler::default();
        let result = sampler.sample(vec![entry]);
        assert_eq!(result[0].status, "unknown");
        assert!(result[0].memory_bytes.is_none());
        let mut entry = this_process();
        entry.record.as_mut().unwrap().process_start_token = None;
        sampler.last_refresh = None;
        let result = sampler.sample(vec![entry]);
        assert_eq!(result[0].status, "unknown");
        assert!(result[0].cpu_percent.is_none());
    }

    #[test]
    #[cfg(windows)]
    fn process_monitor_stop_and_adoption_share_the_windows_birth_identity_gate() {
        let entry = this_process();
        let mut probe = probe_process(entry.pid);
        probe.process_start_token = Some("another-process-at-the-same-path".into());
        assert!(matches!(
            decide(entry.record.as_ref().unwrap(), &probe),
            Adoption::Forget {
                keep_running: true,
                ..
            }
        ));
    }
}

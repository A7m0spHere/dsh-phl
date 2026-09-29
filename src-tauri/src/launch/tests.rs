use super::*;
use std::net::TcpListener;
use std::path::PathBuf;

#[test]
fn deps_selfheal_progress_maps_into_a_monotonic_global_window() {
    // The shape `install_version_deps` really emits: a per-attempt ramp
    // (capped at 0.95) that RESTARTS when a pruned 404 retries, and a
    // final 1.0 on success.
    let phase: Vec<f64> = vec![0.10, 0.55, 0.95, 0.05, 0.80, 0.95, 1.0];
    let mapped: Vec<f64> = phase
        .iter()
        .map(|p| map_launch_phase_progress("install-deps", *p))
        .collect();
    // Inside its window the ramp restarts, so raw mapped values may dip
    // — that is what the launch-side floor absorbs.
    let mut floor = 0.0f64;
    let global: Vec<f64> = mapped
        .iter()
        .map(|m| {
            floor = floor.max(*m);
            floor
        })
        .collect();
    for w in global.windows(2) {
        assert!(w[1] >= w[0], "global launch bar regressed: {global:?}");
    }
    // The window must sit strictly below every later stage's pinned
    // value, or the hand-off regresses at exactly the boundary.
    let deps_ceiling = *global.last().unwrap();
    for later in [0.12, 0.24, 0.30, 0.36, 0.97, 1.0] {
        assert!(
            deps_ceiling < later,
            "install-deps ceiling {deps_ceiling} must stay below the later stage at {later}"
        );
    }
    // Other stages are the global scale already: pass-through.
    for stage in [
        "prepare-home",
        "link-plugins",
        "allocate-port",
        "spawn",
        "await-ready",
    ] {
        assert_eq!(map_launch_phase_progress(stage, 0.42), 0.42);
    }
}

#[test]
fn uncertain_or_live_registration_blocks_a_second_launch() {
    for state in [ProcessState::Alive, ProcessState::Unknown] {
        let processes = Processes::default();
        let registry = Registry::default();
        let rec = PersistedProcess {
            instance_id: "inst".into(),
            pid: 123,
            port: 3099,
            started_at_ms: 1000,
            exe_path: "node.exe".into(),
            process_start_token: Some("live-start".into()),
        };
        registry.remember(rec.clone());
        // Covers a durable record that boot could not adopt into memory.
        let err = ensure_launch_available(&processes, &registry, "inst", |_| registry::Probe {
            state,
            exe_path: Some("node.exe".into()),
            process_start_token: Some("live-start".into()),
            created_at_ms: Some(1000),
        })
        .unwrap_err();
        assert!(err.contains("kept-alive 123 3099"));
        assert_eq!(registry.record_of("inst").unwrap(), rec);
        assert_eq!(
            processes.entry_of("inst").unwrap().pid,
            123,
            "the retry-stop entry must reach the preserved record"
        );
        processes.set(
            "inst",
            ProcessEntry {
                pid: 123,
                port: 3099,
            },
        );
        assert!(ensure_launch_available(&processes, &registry, "inst", |_| {
            registry::Probe::default()
        })
        .is_err());
        assert_eq!(processes.entry_of("inst").unwrap().pid, 123);
        ensure_launch_available(&processes, &registry, "inst", |_| registry::Probe {
            state: ProcessState::Exited,
            ..Default::default()
        })
        .unwrap();
        assert!(processes.entry_of("inst").is_none());
        assert!(registry.record_of("inst").is_none());
    }
}

#[test]
fn reused_registration_is_forgotten_without_touching_the_other_process() {
    let processes = Processes::default();
    let registry = Registry::default();
    registry.remember(PersistedProcess {
        instance_id: "inst".into(),
        pid: 123,
        port: 3099,
        started_at_ms: 1000,
        exe_path: "node.exe".into(),
        process_start_token: Some("old-start".into()),
    });
    ensure_launch_available(&processes, &registry, "inst", |_| registry::Probe {
        state: ProcessState::Alive,
        exe_path: Some("unrelated.exe".into()),
        process_start_token: None,
        created_at_ms: Some(2000),
    })
    .unwrap();
    assert!(registry.record_of("inst").is_none());
}

#[cfg(windows)]
#[test]
fn isolated_home_wins_over_instance_and_provider_case_variants() {
    let mut command = tokio::process::Command::new("node");
    command
        .env("DSH_HOME", "initial")
        .env("dsh_home", "other-instance")
        .env("Dsh_Home", "provider-value");
    set_isolated_home(&mut command, Path::new("C:/isolated-instance/dsh-home"));
    let values: Vec<_> = command
        .as_std()
        .get_envs()
        .filter(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("DSH_HOME"))
        .collect();
    assert_eq!(values.len(), 1);
    assert_eq!(
        values[0].1.unwrap(),
        std::ffi::OsStr::new("C:/isolated-instance/dsh-home")
    );
}

#[tokio::test]
async fn termination_faults_keep_rows_until_exit_is_observed() {
    for kill_ok in [false, true] {
        for state in [
            ProcessState::Alive,
            ProcessState::Unknown,
            ProcessState::Exited,
        ] {
            let processes = Processes::default();
            let registry = Registry::default();
            processes.set(
                "inst",
                ProcessEntry {
                    pid: 123,
                    port: 3099,
                },
            );
            registry.remember(PersistedProcess {
                instance_id: "inst".into(),
                pid: 123,
                port: 3099,
                started_at_ms: 0,
                exe_path: "x".into(),
                process_start_token: None,
            });
            let kill = if kill_ok {
                Ok(())
            } else {
                Err("injected access denied".into())
            };
            let result = finish_termination(
                &processes,
                &registry,
                "inst",
                123,
                kill,
                || state,
                Duration::ZERO,
            )
            .await;
            let exited = state == ProcessState::Exited;
            assert_eq!(result.is_ok(), exited, "kill_ok={kill_ok}, state={state:?}");
            assert_eq!(processes.entry_of("inst").is_none(), exited);
            assert_eq!(registry.record_of("inst").is_none(), exited);
            // A later retry with an actual exit observation is sufficient,
            // even when the kill command still fails against the dead pid.
            finish_termination(
                &processes,
                &registry,
                "inst",
                123,
                Err("already dead".into()),
                || ProcessState::Exited,
                Duration::ZERO,
            )
            .await
            .unwrap();
            assert!(registry.record_of("inst").is_none());
        }
    }
}

#[tokio::test]
async fn kept_child_exit_clears_rows_and_delivers_one_notification() {
    let child = if cfg!(windows) {
        tokio::process::Command::new("cmd")
            .args(["/C", "exit 7"])
            .spawn()
            .unwrap()
    } else {
        tokio::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap()
    };
    let pid = child.id().unwrap();
    let processes = Processes::default();
    let registry = Registry::default();
    processes.set("inst", ProcessEntry { pid, port: 3099 });
    registry.remember(PersistedProcess {
        instance_id: "inst".into(),
        pid,
        port: 3099,
        started_at_ms: 0,
        exe_path: "x".into(),
        process_start_token: None,
    });
    assert!(finish_termination(
        &processes,
        &registry,
        "inst",
        pid,
        Err("injected kill timeout".into()),
        || ProcessState::Unknown,
        Duration::ZERO
    )
    .await
    .is_err());
    let mut events = Vec::new();
    observe_child_exit(child, |status| {
        assert!(forget_exited_process(&processes, &registry, "inst", pid));
        events.push((pid, status.code()));
    })
    .await;
    assert_eq!(events, vec![(pid, Some(7))]);
    assert!(processes.entry_of("inst").is_none());
    assert!(registry.record_of("inst").is_none());
}

#[test]
fn failed_kill_command_is_not_success() {
    #[cfg(windows)]
    let status = {
        use std::os::windows::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(1)
    };
    #[cfg(not(windows))]
    let status = {
        use std::os::unix::process::ExitStatusExt;
        std::process::ExitStatus::from_raw(256)
    };
    let error = check_kill_output(
        123,
        std::process::Output {
            status,
            stdout: vec![],
            stderr: b"access denied".to_vec(),
        },
    )
    .unwrap_err();
    assert!(error.contains("123"));
    assert!(error.contains("access denied"));
}

#[test]
fn port_zero_is_never_considered_free() {
    // `bind(0)` means "any ephemeral port" and always succeeds, so without
    // the explicit guard the allocator could hand DSH `--port 0`.
    assert!(!port_free(0));
}

#[test]
fn auto_allocation_stops_at_the_top_of_the_range() {
    let processes = Processes::default();
    // Starting at 65535 there is nowhere to advance to; the old
    // `wrapping_add` rolled over to 0 and returned it as usable.
    let picked = allocate_port(&processes, "inst", 65535, true);
    // Failing to find a port is a fine outcome here; returning 0 is not.
    if let Ok(port) = picked {
        assert_ne!(port, 0, "port 0 must never be allocated");
    }
}

#[test]
fn auto_allocation_never_lands_on_a_reserved_port() {
    // The adoption regression (2026-09-11): a stored `port: 0` advanced
    // to 1, Windows happily binds 1, and Chromium refuses to open it
    // (`ERR_UNSAFE_PORT`). The scan must resume inside the openable range.
    let processes = Processes::default();
    let picked = allocate_port(&processes, "inst", 0, true).unwrap();
    assert!(picked >= MIN_WEB_PORT, "reserved port allocated: {picked}");
    // A legacy row already poisoned with a reserved port self-heals the
    // same way on the next launch.
    let picked = allocate_port(&processes, "inst", 1, true).unwrap();
    assert!(picked >= MIN_WEB_PORT, "reserved port allocated: {picked}");
}

#[test]
fn explicit_reserved_port_is_refused_with_a_code() {
    // Without the scan to rescue the choice, a configured reserved port
    // must fail the launch loudly — never spawn a DSH nobody can open.
    let processes = Processes::default();
    let err = allocate_port(&processes, "inst", 1, false).unwrap_err();
    assert!(err.contains("保留端口"), "{err}");
    assert!(err.starts_with("[port-conflict]"), "{err}");
    assert!(allocate_port(&processes, "inst", 0, false).is_err());
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-launch-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn web_url_parsed_from_dsh_log_line() {
    let log = "some boot noise\n\
               dsh web: http://127.0.0.1:3080/?token=6qeWvc1o3FEaOTrI4YOOIAP9F_1MSDi4AbNhh2HWO7w\n\
               later output here";
    assert_eq!(
        parse_web_url(log).as_deref(),
        Some("http://127.0.0.1:3080/?token=6qeWvc1o3FEaOTrI4YOOIAP9F_1MSDi4AbNhh2HWO7w")
    );
    // No URL line → None (frontend falls back to bare host:port).
    assert_eq!(parse_web_url("booting…\nready\n"), None);
    // A line that only mentions "http" elsewhere must not be mistaken.
    assert_eq!(parse_web_url("see docs at http://example.com"), None);
}

#[tokio::test]
async fn read_web_url_once_reads_the_head_of_a_settled_log() {
    let dir = std::env::temp_dir().join(format!("phl-weburl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("launch.log");
    // Nothing yet: the wait, not the reader, owns the timing.
    std::fs::write(&path, "").unwrap();
    assert_eq!(read_web_url_once(&path).await, None);
    std::fs::write(&path, "dsh web: http://127.0.0.1:4000/?token=abc123\n").unwrap();
    assert_eq!(
        read_web_url_once(&path).await.as_deref(),
        Some("http://127.0.0.1:4000/?token=abc123")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn command_is_built_for_the_web_entry() {
    let root = temp_dir("cmd");
    std::fs::create_dir_all(root.join("versions/0.1.0/lib")).unwrap();
    std::fs::write(root.join("versions/0.1.0/lib/bin.js"), "// bin").unwrap();
    // The fake runtime lives where `build_command` looks for it on THIS
    // platform: the zip layout's top level on Windows, `bin/` elsewhere.
    let fake_node = process::runtime_bin_dir(&root, "node-22").join(node_binary());
    std::fs::create_dir_all(fake_node.parent().unwrap()).unwrap();
    std::fs::write(&fake_node, "bin").unwrap();

    let dsh_home = root.join("instances/i/dsh-home");
    let (program, args, env) = build_command(
        &root,
        "0.1.0",
        "node-22",
        3081,
        &["--verbose".into()],
        &dsh_home,
    )
    .unwrap();

    assert!(program.to_string_lossy().ends_with(node_binary()));
    // User args first, then our --port / --no-open — a duplicate --port
    // from the instance config can never override PHL's allocation.
    let expected_bin = root
        .join("versions")
        .join("0.1.0")
        .join("lib")
        .join("bin.js");
    assert_eq!(
        args,
        vec![
            expected_bin.to_string_lossy().into_owned(),
            "web".into(),
            "--verbose".into(),
            "--port".into(),
            "3081".into(),
            "--no-open".into(),
        ]
    );
    assert_eq!(
        env.iter()
            .find(|(k, _)| k == "DSH_HOME")
            .map(|(_, v)| v.as_str()),
        Some(dsh_home.to_string_lossy().as_ref())
    );
    // The runtime bin dir leads PATH so child processes resolve this node.
    let path = &env.iter().find(|(k, _)| k == "PATH").unwrap().1;
    let entries: Vec<_> = std::env::split_paths(std::ffi::OsStr::new(path)).collect();
    assert_eq!(
        entries.first(),
        Some(&process::runtime_bin_dir(&root, "node-22"))
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_version_or_runtime_is_refused_up_front() {
    let root = temp_dir("missing");
    std::fs::create_dir_all(root.join("versions/0.1.0/lib")).unwrap();
    std::fs::write(root.join("versions/0.1.0/lib/bin.js"), "// bin").unwrap();

    let dsh_home = root.join("instances/i/dsh-home");
    let err = build_command(&root, "0.1.0", "node-22", 3080, &[], &dsh_home).unwrap_err();
    assert!(err.contains("Runtime node-22 未安装"), "{err}");

    let fake_node = process::runtime_bin_dir(&root, "node-22").join(node_binary());
    std::fs::create_dir_all(fake_node.parent().unwrap()).unwrap();
    std::fs::write(&fake_node, "bin").unwrap();
    std::fs::remove_dir_all(root.join("versions/0.1.0")).unwrap();
    let err = build_command(&root, "0.1.0", "node-22", 3080, &[], &dsh_home).unwrap_err();
    assert!(err.contains("lib/bin.js"), "{err}");

    let _ = std::fs::remove_dir_all(&root);
}

/// The system runtime is the one runtime PHL does not own, so launch has to
/// *find* it — and the answer it finds has to be the same one the runtime
/// page reports and the resolver verified (2026-09-22 review). A bare
/// `node` used to be handed through here, which the OS resolves at spawn
/// time; that is precisely the resolution a Finder-launched app cannot do.
#[test]
fn system_runtime_runs_the_resolved_node() {
    let root = temp_dir("sys");
    std::fs::create_dir_all(root.join("versions/0.1.0/lib")).unwrap();
    std::fs::write(root.join("versions/0.1.0/lib/bin.js"), "// bin").unwrap();

    let resolved = crate::discovery::inspect::resolve_system_node();
    let built = build_command(&root, "0.1.0", "node-system", 3080, &[], &root.join("h"));
    match resolved {
        // This host has a Node: launch runs that exact binary, absolutely.
        Some((node, _)) => {
            let (program, _, env) = built.unwrap();
            assert_eq!(
                program, node,
                "launch must run the Node the resolver verified"
            );
            assert!(program.is_absolute(), "{program:?}");
            let path = env.iter().find(|(k, _)| k == "PATH").unwrap().1.as_str();
            let entries: Vec<_> = std::env::split_paths(std::ffi::OsStr::new(path)).collect();
            assert_eq!(entries.first().map(|path| path.as_path()), node.parent());
        }
        // No Node anywhere: refuse with the actionable error instead of a
        // bare name that cannot spawn. (Resolution itself is pinned by
        // discovery::inspect's own tests.)
        None => {
            let err = built.unwrap_err();
            assert!(err.contains("系统 Node 不可用"), "{err}");
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn system_node_is_reachable_from_a_narrow_child_path_and_keeps_home_isolation() {
    let Some((node, expected_version)) = crate::discovery::inspect::resolve_system_node() else {
        // Hosts without Node exercise the resolver's failure path elsewhere.
        return;
    };
    let node_dir = node.parent().expect("resolved Node has a parent");
    let retained_path = std::env::temp_dir().join("phl-existing-path-entry");
    let narrow = std::env::join_paths([&retained_path]).unwrap();
    let child_path = process::prepend_path_dir(node_dir, Some(&narrow)).unwrap();
    let entries: Vec<_> = std::env::split_paths(&child_path).collect();
    assert_eq!(entries.first(), Some(&node_dir.to_path_buf()));
    assert!(entries.contains(&retained_path));

    // Invoke `node` by name in a process with no inherited environment. This
    // proves the child PATH itself is enough to find the exact Node PHL chose.
    #[cfg(windows)]
    let mut child = {
        let command = std::env::var_os("ComSpec")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\cmd.exe"));
        std::process::Command::new(command)
    };
    #[cfg(windows)]
    child.args(["/C", "node", "--version"]);
    #[cfg(not(windows))]
    let mut child = std::process::Command::new("/bin/sh");
    #[cfg(not(windows))]
    child.args(["-c", "node --version"]);
    let output = child
        .env_clear()
        .env("PATH", &child_path)
        .env("DSH_HOME", "/temporary/phl-isolated-home")
        .output()
        .expect("system Node must be resolvable through the child PATH");
    assert!(output.status.success());
    let actual_version = String::from_utf8_lossy(&output.stdout)
        .trim()
        .trim_start_matches('v')
        .to_string();
    assert_eq!(actual_version, expected_version);

    let mut command = tokio::process::Command::new(&node);
    command
        .env("PATH", &child_path)
        .env("DSH_HOME", "attempted-override");
    let isolated_home = std::env::temp_dir().join("phl-isolated-home");
    set_isolated_home(&mut command, &isolated_home);
    let configured: Vec<_> = command.as_std().get_envs().collect();
    let path = configured
        .iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .and_then(|(_, value)| *value)
        .expect("PATH remains configured");
    assert_eq!(path, child_path.as_os_str());
    let home = configured
        .iter()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("DSH_HOME"))
        .and_then(|(_, value)| *value)
        .expect("DSH_HOME remains configured");
    assert_eq!(home, isolated_home.as_os_str());
}

#[test]
fn instance_path_precedes_but_does_not_hide_the_resolved_system_node() {
    let custom = std::env::temp_dir().join("phl-instance-custom-path");
    let node = std::env::temp_dir().join("phl-resolved-node-bin");
    let inherited = std::env::temp_dir().join("phl-inherited-path");
    let fallback = std::env::join_paths([&node, &inherited]).unwrap();
    let custom_only = std::env::join_paths([&custom, &node]).unwrap();
    let merged = process::merge_path_values(&custom_only, &fallback).unwrap();
    let entries: Vec<_> = std::env::split_paths(&merged).collect();
    assert_eq!(entries, vec![custom, node, inherited]);
}

#[test]
fn port_allocation_skips_phl_and_os_conflicts() {
    let processes = Processes::default();
    processes
        .0
        .lock()
        .unwrap()
        .insert("other".into(), ProcessEntry { pid: 1, port: 3081 });

    // Fixed port colliding with a PHL instance → next free port under auto…
    assert_eq!(allocate_port(&processes, "me", 3081, true).unwrap(), 3082);
    // …but refused as-is when auto is off.
    let err = allocate_port(&processes, "me", 3081, false).unwrap_err();
    assert!(err.contains("已被实例 other 占用"), "{err}");

    // An OS-level listener is skipped by the bind test too.
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let occupied = listener.local_addr().unwrap().port();
    let picked = allocate_port(&processes, "me", occupied, true).unwrap();
    assert_ne!(picked, occupied);
    assert!(port_free(picked));

    // The instance's own recorded port (restart-in-place) is not a conflict,
    // provided the OS also considers it free.
    let own = allocate_port(&processes, "me", 20_000, true).unwrap();
    processes
        .0
        .lock()
        .unwrap()
        .insert("me".into(), ProcessEntry { pid: 2, port: own });
    assert_eq!(allocate_port(&processes, "me", own, true).unwrap(), own);

    drop(listener);
}

#[test]
fn log_tail_keeps_the_last_lines() {
    let dir = temp_dir("tail");
    let path = dir.join("launch.log");
    std::fs::write(&path, "l1\nl2\nl3\n").unwrap();
    let tail = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(log_tail(&path, 2));
    assert_eq!(tail, "l2\nl3");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_webui_token_never_leaves_the_backend() {
    // `dsh web` prints the authenticated URL at the top of the log, and a
    // short log puts that line inside the tail PHL shows the user.
    let line =
        "dsh web: http://127.0.0.1:3080/?token=6qeWvc1o3FEaOTrI4YOOIAP9F_1MSDi4AbNhh2HWO7w\nready";
    let redacted = redact_web_token(line);
    assert!(!redacted.contains("6qeWvc"), "{redacted}");
    assert!(redacted.contains("token=<redacted>"));
    assert!(redacted.contains("ready"), "the rest of the log survives");

    // A token that ends at a query separator keeps the rest of the query.
    assert_eq!(
        redact_web_token("http://h/?token=abc&x=1"),
        "http://h/?token=<redacted>&x=1"
    );
    assert_eq!(redact_web_token("nothing to hide"), "nothing to hide");

    // And the tail path itself redacts, not just the helper.
    let dir = temp_dir("tail-token");
    let path = dir.join("launch.log");
    std::fs::write(&path, format!("{line}\n")).unwrap();
    let tail = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(log_tail(&path, 10));
    assert!(!tail.contains("6qeWvc"), "{tail}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn log_tail_of_a_huge_file_reads_only_the_window() {
    // O-12: asking for 3 lines out of a log that has megabytes of
    // noise must not materialise the whole file. Write well past the
    // tail window and check only the last three lines come back.
    let dir = temp_dir("tail-big");
    let path = dir.join("launch.log");
    let mut raw = vec![b'x'; process::LOG_TAIL_WINDOW as usize + 4096];
    raw.extend_from_slice(b"\nline-a\nline-b\nline-c\n");
    std::fs::write(&path, &raw).unwrap();
    let tail = log_tail(&path, 3).await;
    assert_eq!(tail, "line-a\nline-b\nline-c");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn pruning_keeps_the_newest_logs_and_drops_the_rest() {
    let dir = temp_dir("prune");
    for i in 0..5 {
        std::fs::write(dir.join(format!("launch-2026-09-06T0{i}-00-00.log")), b"x").unwrap();
    }
    std::fs::write(dir.join("other.log"), b"x").unwrap();
    process::prune_launch_logs(&dir, 2).await;
    assert!(dir.join("launch-2026-09-06T04-00-00.log").exists());
    assert!(dir.join("launch-2026-09-06T03-00-00.log").exists());
    assert!(!dir.join("launch-2026-09-06T00-00-00.log").exists());
    assert!(dir.join("other.log").exists(), "only launch-*.log is swept");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stale_pid_never_removes_the_fresh_process_entry() {
    // The relaunch race, pinned: stop + instant re-launch puts a new pid
    // in the map; the old watcher's delayed removal must not forget it
    // (that would orphan the live process from PHL's bookkeeping).
    let processes = Processes::default();
    processes.set(
        "inst",
        ProcessEntry {
            pid: 111,
            port: 3080,
        },
    );
    processes.remove_if_pid("inst", 999); // unrelated stale pid
    assert_eq!(
        processes.entry_of("inst").unwrap().pid,
        111,
        "stale removal refused"
    );
    processes.remove_if_pid("inst", 111);
    assert!(
        processes.entry_of("inst").is_none(),
        "the owning pid still removes"
    );
    // Removal after disappearance is inert, not a panic.
    processes.remove_if_pid("inst", 111);
}

#[tokio::test]
async fn termination_is_forgotten_only_once_exit_is_confirmed() {
    // R3's gate, exercised against real process states: a pid the kernel
    // reports gone passes; a live process whose kill_tree succeeds passes;
    // either way the in-memory map and the persisted row move together.
    let processes = Processes::default();
    let registry = Registry::default();
    let mut gone = if cfg!(windows) {
        tokio::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap()
    } else {
        tokio::process::Command::new("true").spawn().unwrap()
    };
    let pid = gone.id().unwrap();
    gone.wait().await.unwrap();
    processes.set("inst", ProcessEntry { pid, port: 3099 });
    registry.remember(PersistedProcess {
        instance_id: "inst".into(),
        pid,
        port: 3099,
        started_at_ms: 0,
        exe_path: "x".into(),
        process_start_token: None,
    });
    terminate_and_forget(
        &processes,
        &registry,
        "inst",
        pid,
        3099,
        "启动已取消",
        &mut gone,
    )
    .await
    .unwrap();
    assert!(processes.entry_of("inst").is_none());
    assert!(
        registry.record_of("inst").is_none(),
        "confirmed exit: forgotten"
    );

    // A live child that *can* be terminated also clears both rows — the
    // proof is the probe saying dead within the confirmation window.
    let mut sleeper = if cfg!(windows) {
        tokio::process::Command::new("cmd")
            .args(["/C", "ping -n 60 127.0.0.1 > nul"])
            .spawn()
            .unwrap()
    } else {
        tokio::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap()
    };
    let spid = sleeper.id().unwrap();
    processes.set(
        "alive",
        ProcessEntry {
            pid: spid,
            port: 3100,
        },
    );
    registry.remember(PersistedProcess {
        instance_id: "alive".into(),
        pid: spid,
        port: 3100,
        started_at_ms: 0,
        exe_path: "x".into(),
        process_start_token: None,
    });
    terminate_and_forget(
        &processes,
        &registry,
        "alive",
        spid,
        3100,
        "等待 120 秒仍未就绪",
        &mut sleeper,
    )
    .await
    .unwrap();
    assert!(
        registry.record_of("alive").is_none(),
        "kill confirmed dead: registration cleared"
    );
    assert!(processes.entry_of("alive").is_none());
    let _ = sleeper.wait().await;
}

/// The stop path end to end with a real process group rather than an
/// injected probe: the tree goes down and the rows are forgotten *because*
/// the exit was observed. `termination_is_forgotten_only_once_exit_is_confirmed`
/// pins the policy; this pins that the Unix signals actually reach a real
/// child (2026-09-22 review: `kill -9 <pid>` reached nothing else).
#[cfg(unix)]
#[tokio::test]
async fn a_stop_takes_a_real_tree_down_and_forgets_the_rows() {
    let processes = Processes::default();
    let registry = Registry::default();
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg("sleep 60")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // What the launch path does, so this is the group a stop may signal.
    command.process_group(0);
    let child = command.spawn().unwrap();
    let pid = child.id().unwrap();
    processes.set("inst", ProcessEntry { pid, port: 3099 });
    registry.remember(PersistedProcess {
        instance_id: "inst".into(),
        pid,
        port: 3099,
        started_at_ms: 0,
        exe_path: "x".into(),
        process_start_token: None,
    });
    assert_eq!(probe_process(pid).state, ProcessState::Alive);

    terminate_or_gone(&processes, &registry, "inst", pid)
        .await
        .unwrap();

    assert!(processes.entry_of("inst").is_none());
    assert!(
        registry.record_of("inst").is_none(),
        "observed exit: forgotten"
    );
    assert_eq!(probe_process(pid).state, ProcessState::Exited);
}

#[tokio::test]
async fn a_stop_of_a_process_that_died_after_the_keep_clears_the_rows() {
    // The kept-alive instance's retry-stop path (R3): `terminate_or_gone`
    // must treat a kill failure against an already-dead pid as the desired
    // outcome and clear both rows. Whether taskkill itself succeeds or
    // errors on a dead pid varies by state, so the assertion is the END
    // — rows gone, command Ok — whichever branch the OS took.
    let processes = Processes::default();
    let registry = Registry::default();
    let mut gone = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let pid = gone.id();
    gone.wait().unwrap();
    processes.set("inst", ProcessEntry { pid, port: 3099 });
    registry.remember(PersistedProcess {
        instance_id: "inst".into(),
        pid,
        port: 3099,
        started_at_ms: 0,
        exe_path: "x".into(),
        process_start_token: None,
    });
    // `stop_permission`'s decide() gate accepts a dead pid, and the stop
    // core clears the rows against it.
    assert!(stop_permission(&registry, "inst", pid).is_ok());
    terminate_or_gone(&processes, &registry, "inst", pid)
        .await
        .expect("a dead pid must never block the stop retry");
    assert!(processes.entry_of("inst").is_none());
    assert!(registry.record_of("inst").is_none());
}

#[test]
fn stop_refuses_a_pid_the_registry_does_not_own() {
    let registry = Registry::default();
    // A genuinely dead pid stands in for "already gone". The old
    // hardcoded 1234 was proof-of-absence only on Windows: macOS recycles
    // low pids fast, so a system daemon owned the number and the probe
    // (rightly) refused to treat it as ours.
    let mut gone = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap()
    } else {
        std::process::Command::new("true").spawn().unwrap()
    };
    let dead_pid = gone.id();
    gone.wait().unwrap();
    // No record: a process PHL launched in this session is ours.
    assert!(stop_permission(&registry, "inst", dead_pid).is_ok());

    // A record whose creation stamp cannot belong to this pid: the number
    // has been reused, so PHL must refuse to kill it.
    registry.remember(PersistedProcess {
        instance_id: "inst".into(),
        pid: dead_pid,
        port: 3080,
        started_at_ms: 0,
        exe_path: "C:\\node.exe".into(),
        process_start_token: None,
    });
    let err = stop_permission(&registry, "inst", std::process::id()).unwrap_err();
    assert!(err.contains("身份无法确认"), "{err}");

    // A pid that is already gone is not a kill target, but must not block
    // the cleanup either.
    assert!(stop_permission(&registry, "inst", dead_pid).is_ok());
}

#[test]
fn a_watcher_only_closes_the_window_of_its_own_process() {
    // The other half of the relaunch race: the old watcher must not close
    // the WebUI window that belongs to the new process, while a plain stop
    // (which removes the entry itself) still has to close it.
    let processes = Processes::default();
    processes.set(
        "inst",
        ProcessEntry {
            pid: 111,
            port: 3080,
        },
    );
    assert!(processes.is_current_or_empty("inst", 111), "its own pid");

    // A relaunch registered the new pid before the old watcher woke up.
    processes.set(
        "inst",
        ProcessEntry {
            pid: 222,
            port: 3081,
        },
    );
    assert!(
        !processes.is_current_or_empty("inst", 111),
        "the stale watcher must leave the new window alone"
    );
    assert!(processes.is_current_or_empty("inst", 222));

    // Stop removes the entry itself; "nothing registered" is not "newer".
    processes.remove_if_pid("inst", 222);
    assert!(processes.is_current_or_empty("inst", 222));
    assert!(processes.is_current_or_empty("never-seen", 1));
}

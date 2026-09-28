//! Real Runtime coexistence, opt-in: two real Node runtimes installed through
//! PHL's own pipeline, each hosting its own real DSH instance at the same
//! time — and the process table proof that each instance really runs the Node
//! it was bound to.
//!
//! The reviewer's release list names "真实 Runtime 并存" separately from the
//! DSH version coexistence, because they are different claims: the fake lanes
//! prove PHL's binding logic, only this proves two *real* Node trees can host
//! two *real* DSH servers side by side on one machine.
//!
//! Run it explicitly (opt-in, network required):
//!
//! ```text
//! PHL_REAL_DSH=1 cargo test --lib real_runtime -- --ignored --nocapture --test-threads=1
//! ```

use std::collections::HashMap;

use super::super::release_e2e::driver::{launch, with_cleanup, Session};
use super::super::release_e2e::fixtures::NodeInfo;
use super::real_dsh::{create_real_instance, install_real, opted_in, REGISTRY, TARGET_VERSION};

/// The Node dist index the app itself uses (settings' official source).
const NODE_DIST: &str = "https://nodejs.org/dist";

/// Two majors that must both exist in the dist index: the pinned pair is
/// resolved from the real catalogue rather than hard-coded patch versions.
const MAJORS: [u32; 2] = [22, 24];

async fn install_runtime(session: &Session, version_name: &str, version: &str) {
    let id = format!("real-runtime-{version_name}");
    let flag = session.transfers.take(&id);
    let task = crate::resources::Tasks::default()
        .begin(
            crate::resources::TaskInfo::new(
                id.clone(),
                "runtime-install",
                format!("安装 Runtime {version_name}"),
                &[],
            ),
            Some(flag.clone()),
        )
        .unwrap();
    let result = crate::runtimes::install::run_runtime_install(
        &flag,
        NODE_DIST,
        version_name,
        version,
        &session.root(),
        false,
        &task,
        &crate::release_e2e::journeys::progress_channel(),
    )
    .await;
    session.transfers.release(&id);
    result.unwrap_or_else(|e| panic!("runtime {version_name} install failed: {e}"));
}

/// The executable a live pid is actually running (Windows: the image path).
fn process_image(pid: u32) -> String {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid} -ErrorAction Stop).Path"),
        ])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[tokio::test]
#[ignore = "real Runtime coexistence: needs network + npm; opt-in via PHL_REAL_DSH=1"]
async fn real_runtimes_host_real_dsh_side_by_side() {
    if !opted_in() {
        println!(
            "SKIP real_runtimes_host_real_dsh_side_by_side: set PHL_REAL_DSH=1 to run \
             (downloads two Node runtimes from {NODE_DIST} and one real DSH from {REGISTRY})"
        );
        return;
    }
    let Some(host_node) = NodeInfo::discover() else {
        panic!("real runtime acceptance: no `node` on PATH");
    };

    let session = Session::fresh("real-runtimes");
    with_cleanup(&session, async {
        // 1. Resolve two real runtimes from the live dist index and install
        //    both through PHL's pipeline (download → SHASUMS verify → extract
        //    → health gate → promote).
        let index = crate::runtimes::list_node_runtimes(NODE_DIST.to_string())
            .await
            .expect("node dist index reachable");
        let mut chosen = Vec::new();
        for major in MAJORS {
            let entry = index
                .iter()
                .find(|m| m.major == major as u64 && m.lts)
                .or_else(|| index.iter().find(|m| m.major == major as u64))
                .unwrap_or_else(|| panic!("no Node {major} in the dist index"));
            chosen.push((format!("node-{}", entry.version), entry.version.clone()));
        }
        for (name, version) in &chosen {
            install_runtime(&session, name, version).await;
            let dir = session.root().join("runtimes").join(name);
            assert!(
                dir.join(crate::launch::node_binary()).exists(),
                "{name}: binary"
            );
            assert!(dir.join("phl-runtime.json").exists(), "{name}: marker");
            // The installed tree really reports its own version.
            let out = std::process::Command::new(dir.join(crate::launch::node_binary()))
                .arg("--version")
                .output()
                .expect("installed node runs");
            let reported = String::from_utf8_lossy(&out.stdout).trim().to_string();
            assert_eq!(
                reported,
                format!("v{version}"),
                "{name} reports its own version"
            );
        }
        println!(
            "runtimes: {} and {} installed and healthy",
            chosen[0].0, chosen[1].0
        );

        // 2. One real DSH version, two instances, one runtime each.
        let catalog = crate::versions::list_dsh_versions(REGISTRY.to_string())
            .await
            .expect("catalog");
        let entry = catalog
            .iter()
            .find(|v| v.name == TARGET_VERSION)
            .expect("target version published");
        let src = entry.source.as_ref().unwrap();
        install_real(
            &session,
            &host_node.exe,
            TARGET_VERSION,
            &src.tarball,
            src.integrity.as_deref(),
        )
        .await;

        let mut launched = Vec::new();
        for (idx, (runtime, _)) in chosen.iter().enumerate() {
            let id = format!("rt-{}", idx + 1);
            let dir = create_real_instance(&session, &id, &format!("dsh-{TARGET_VERSION}")).await;
            // Bind the instance to THIS runtime (the manifest is the binding
            // the launcher reads).
            let mut manifest = crate::instances::load_manifest(&dir, &id).await.unwrap();
            manifest.runtime_id = runtime.clone();
            crate::instances::manifest::write_manifest(&dir, &manifest)
                .await
                .unwrap();

            let held = crate::release_e2e::driver::lock_instance(&session.locks, &id);
            let outcome = launch(
                &session,
                &held,
                &format!("{id}-launch"),
                &id,
                TARGET_VERSION,
                runtime,
                REGISTRY,
                0,
                true,
                HashMap::new(),
                Vec::new(),
                "web",
                &crate::release_e2e::journeys::progress_channel(),
            )
            .await
            .unwrap_or_else(|e| panic!("{id} on {runtime} must launch: {e}"));
            assert!(outcome.outcome.web_url.is_some(), "{id}: authenticated URL");
            launched.push((
                id,
                runtime.clone(),
                outcome.outcome.pid,
                outcome.outcome.port,
            ));
        }

        // 3. Both are live at once, each on its OWN Node binary — the process
        //    table is the proof, not PHL's own bookkeeping.
        for (id, runtime, pid, port) in &launched {
            let image = process_image(*pid);
            let expected = session
                .root()
                .join("runtimes")
                .join(runtime)
                .join(crate::launch::node_binary());
            assert!(
                image.eq_ignore_ascii_case(&expected.to_string_lossy()),
                "{id} (pid {pid}) must run {runtime}'s node, got {image}"
            );
            assert!(
                crate::release_e2e::port_listening(*port).await,
                "{id} serves on :{port}"
            );
            println!("{id}: {runtime} (pid {pid}) serving on :{port}");
        }
        assert_ne!(launched[0].2, launched[1].2, "two distinct processes");

        for (id, _, _, port) in &launched {
            session.terminate(id).await;
            assert!(
                crate::release_e2e::wait_until(10_000, || crate::launch::port_free(*port)).await,
                "{id}: port frees after stop"
            );
        }
        println!("STOP: both runtime-bound instances stopped, both ports free");
    })
    .await;
}

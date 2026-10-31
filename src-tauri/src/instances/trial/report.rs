//! The trial-upgrade result report (Issue #8).
//!
//! The trial pipeline proves a copy can be BUILT; the upgrade question is
//! whether the copy can RUN and whether the plugins it carries still load.
//! This module reads the copy's own artifacts — the trial marker, the
//! persisted plan, the latest launch log, the profile's plugin tree — and
//! produces one honest page:
//!
//! * never conflates "启动成功" with "模型和插件全部可用": the WebUI verdict
//!   is evidence-based (token URL in the log + a live port probe), plugin
//!   error lines are a REDACTED heuristic excerpt (an indicator, not a
//!   diagnosis), and everything that could not be observed says
//!   "unverified" instead of passing;
//! * the source instance is never touched — this module is read-only on the
//!   source and only writes the report file into the COPY's own directory;
//! * `engines.dsh` verdicts ride along from `plugins::engines` (#10), so
//!   the report answers "这些插件对目标版本自述兼容吗".

use std::path::Path;

use serde::Serialize;
use tauri::State;

use super::marker::read_trial_marker;

/// Heuristic keyword set for "this log line deserves a human's eyes". NOT a
/// diagnosis: DSH may print benign lines containing them, which is exactly
/// why the report labels the excerpt as an indicator.
const ERROR_LINE_KEYWORDS: &[&str] = &["error", "panic", "失败", "has no create()"];

/// Cap on the heuristic excerpt so one noisy boot cannot flood the report.
const MAX_ERROR_LINES: usize = 10;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub count: usize,
    pub unreadable: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrialUpgradeReport {
    pub trial_id: String,
    /// The copy's source instance, from the persisted plan (`None` when the
    /// plan record no longer exists — reported, not guessed).
    pub source_id: Option<String>,
    pub plan_id: Option<String>,
    /// Bare target version the copy binds (from the trial marker).
    pub target_version: String,
    pub target_runtime_id: String,
    pub generated_at: String,
    /// `pass` | `partial` | `fail` | `unverified` — see `overall_of`.
    pub overall: String,
    /// True when at least one launch log exists: the copy HAS been started.
    pub launched_ever: bool,
    /// `pass` | `partial` | `unverified` — evidence-backed WebUI verdict.
    pub webui: String,
    pub webui_detail: String,
    /// Heuristic error-ish lines from the latest launch log (token-redacted,
    /// capped). An indicator for a human, never an automated diagnosis.
    pub error_lines: Vec<String>,
    pub exit_note: String,
    /// Per-plugin `engines.dsh` verdicts vs the copy's target version (#10).
    pub plugins: Vec<crate::plugins::engines::PluginEngineCompat>,
    /// Commit-time carry-overs, restated so one page tells the whole story.
    pub link_failures: Vec<String>,
    pub mismatched_packages: Vec<String>,
    /// Present only for `config+sessions` copies.
    pub sessions: Option<SessionSummary>,
    /// Always stated on the page: the trial pipeline never writes to the
    /// source instance (the copy pipeline's structural guarantee).
    pub source_note: String,
    /// Where the one-pager was persisted (inside the copy's own directory).
    pub report_path: Option<String>,
}

/// `overall` from the observable evidence. `pass` here means exactly "the
/// copy started and the WebUI answered" — the page separately says what was
/// NOT verified (model availability, real task runs, third-party plugin
/// runtime health).
fn overall_of(webui: &str, error_lines: &[String]) -> &'static str {
    if !error_lines.is_empty() {
        return "fail";
    }
    match webui {
        "pass" => "pass",
        "partial" => "partial",
        _ => "unverified",
    }
}

/// (webui verdict, error lines) from one launch log's text + a live port
/// probe. Pure over its inputs so the table tests cover the matrix.
fn webui_evidence(
    log_text: Option<&str>,
    port_reachable: bool,
    port: u16,
) -> (String, String, Vec<String>) {
    let mut errors = Vec::new();
    if let Some(text) = log_text {
        for line in text.lines() {
            let lower = line.to_lowercase();
            if ERROR_LINE_KEYWORDS.iter().any(|k| lower.contains(k)) {
                errors.push(crate::launch::redact_web_token(line.trim()));
                if errors.len() >= MAX_ERROR_LINES {
                    break;
                }
            }
        }
    }
    let has_token_url = match log_text {
        Some(text) => crate::launch::parse_web_url(text).is_some() || text.contains("token="),
        None => false,
    };
    let (webui, detail) = match (port_reachable, has_token_url) {
        (true, true) => (
            "pass",
            format!("端口 {port} 可达，且启动日志包含本次 boot 的 token URL"),
        ),
        (true, false) => (
            "partial",
            format!("端口 {port} 可达，但日志中未找到 token URL（0.2.x 输出格式变化？）"),
        ),
        (false, _) => (
            "unverified",
            "副本当前未在运行，无法验证 WebUI；启动后重新生成报告即可补验".to_string(),
        ),
    };
    (webui.to_string(), detail, errors)
}

/// Live, best-effort reachability probe of the copy's WebUI port. A refused
/// or timed-out connect is "not running", NOT an error — the report must be
/// generable for a stopped copy.
async fn port_reachable(port: u16) -> bool {
    let addr = ("127.0.0.1", port);
    tokio::time::timeout(std::time::Duration::from_millis(500), async move {
        tokio::net::TcpStream::connect(addr).await
    })
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// Command: build (and persist into the copy's own directory) the one-page
/// upgrade report for a trial copy. Read-only on everything else; refuses
/// non-trial instances instead of guessing.
#[tauri::command]
pub async fn trial_upgrade_report(
    phl: State<'_, crate::paths::PhlState>,
    instance_id: String,
) -> Result<TrialUpgradeReport, String> {
    let root = phl.root();
    let dest = crate::instances::instance_dir(&root, &instance_id)?;
    let marker = read_trial_marker(&dest)
        .await
        .ok_or_else(|| "该实例不是试用副本（没有 phl-trial.json），无法生成升级报告".to_string())?;
    build_report(&root, &dest, &instance_id, &marker).await
}

pub(crate) async fn build_report(
    root: &Path,
    dest: &Path,
    instance_id: &str,
    marker: &super::marker::TrialMarker,
) -> Result<TrialUpgradeReport, String> {
    let manifest = crate::instances::load_manifest(dest, instance_id).await?;
    let plan = super::plan::load_trial_plan(root, &marker.plan_id).await;

    // Launch evidence: the newest log under the copy's own logs/ dir.
    let logs_dir = dest.join("logs");
    let latest_log = crate::launch::latest_launch_log(&logs_dir).await;
    let log_text = match &latest_log {
        Some(path) => tokio::fs::read_to_string(path).await.ok(),
        None => None,
    };
    let launched_ever = latest_log.is_some();
    // Manifest stores the port as u32 but a real TCP port is u16; the
    // fallback (0) can never be reachable, which degrades honestly.
    let port = u16::try_from(manifest.port).unwrap_or(0);
    let reachable = port_reachable(port).await;
    let (webui, webui_detail, error_lines) = webui_evidence(log_text.as_deref(), reachable, port);
    let exit_note = if reachable {
        format!("当前正在运行（端口 {}）", manifest.port)
    } else if launched_ever {
        "最近一次启动已结束；退出原因见日志摘录（PHL 不猜测原因）".to_string()
    } else {
        "尚未启动过；启动后重新生成报告即可补验".to_string()
    };

    // Plugins as they are NOW on the copy, with their own engines.dsh claim
    // evaluated against the TARGET version (#10).
    let profile = crate::instances::profile_dir(root, instance_id).await?;
    let plugins_scan = crate::instances::scan_plugins(&profile).await;
    let plugins = crate::plugins::engines::compat_for_installed(
        &plugins_scan,
        &profile,
        crate::plugins::engines::bare_dsh_version(&marker.target_version),
    )
    .await;

    // Sessions only exist on `config+sessions` copies.
    let sessions = if marker.scope == "config+sessions" {
        let home = crate::instances::home_of(dest, &manifest);
        let listing = tokio::task::spawn_blocking(move || crate::sessions::list_in_home(&home))
            .await
            .map_err(|e| e.to_string())?;
        Some(SessionSummary {
            count: listing.sessions.len(),
            unreadable: listing
                .unreadable
                .iter()
                .map(|u| format!("{}: {}", u.session_dir, u.reason))
                .collect(),
        })
    } else {
        None
    };

    // The one-pager is persisted into the copy's own directory so it can be
    // read back (and attached to a bug report) without re-deriving anything.
    let report_path = dest.join("phl-trial-report.json");
    let report = TrialUpgradeReport {
        trial_id: instance_id.to_string(),
        source_id: plan.as_ref().map(|p| p.source_id.clone()),
        plan_id: Some(marker.plan_id.clone()),
        target_version: marker.target_version.clone(),
        target_runtime_id: marker.target_runtime_id.clone(),
        generated_at: crate::versions::now_iso(),
        overall: overall_of(&webui, &error_lines).to_string(),
        launched_ever,
        webui,
        webui_detail,
        error_lines,
        exit_note,
        plugins,
        link_failures: marker.link_failures.clone(),
        mismatched_packages: marker.mismatched_packages.clone(),
        sessions,
        source_note: "试用通路从不写入源实例：复制发生在独立 HOME，源 manifest/会话/插件原样保留"
            .to_string(),
        report_path: None,
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    crate::paths::write_durable(&report_path, &bytes)?;
    Ok(TrialUpgradeReport {
        report_path: Some(report_path.to_string_lossy().into_owned()),
        ..report
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::manifest::{write_manifest, InstanceManifest};
    use crate::instances::ManagementMode;

    fn manifest(id: &str, port: u32) -> InstanceManifest {
        InstanceManifest {
            schema_version: 2,
            id: id.into(),
            name: "trial copy".into(),
            note: None,
            kind: "sandbox".into(),
            hue: 0,
            version_id: "dsh-0.2.1-rc.5".into(),
            runtime_id: "node-system".into(),
            port,
            auto_port: false,
            profile: "web".into(),
            created_at: "now".into(),
            last_run_at: None,
            total_runtime: 0,
            favorite: false,
            env: Default::default(),
            args: Vec::new(),
            api: None,
            management_mode: ManagementMode::ManagedCopy,
            source: Default::default(),
            external_home: None,
            adopted_from: None,
        }
    }

    fn marker(plan_id: &str, scope: &str) -> super::super::marker::TrialMarker {
        super::super::marker::TrialMarker {
            plan_id: plan_id.into(),
            target_version: "0.2.1-rc.5".into(),
            target_runtime_id: "node-system".into(),
            scope: scope.into(),
            workspace: "fresh".into(),
            allocated_port: 0,
            readiness: "readyToLaunch".into(),
            link_failures: vec!["link-x".into()],
            mismatched_packages: vec!["pkg-y".into()],
        }
    }

    #[test]
    fn webui_evidence_table() {
        let url_line = "dsh web: http://127.0.0.1:8021/?token=abc123";
        let (webui, detail, errors) = webui_evidence(Some(url_line), true, 8021);
        assert_eq!(webui, "pass");
        assert!(detail.contains("8021"));
        assert!(errors.is_empty());

        // Reachable port but no URL anywhere in the log → partial, not pass.
        let (webui, _, _) = webui_evidence(Some("listening on 8021"), true, 8021);
        assert_eq!(webui, "partial");

        // Has a URL but nothing listening → unverified (honest, not a guess).
        let (webui, detail, _) = webui_evidence(Some(url_line), false, 8021);
        assert_eq!(webui, "unverified");
        assert!(detail.contains("重新生成"), "{detail}");

        // No log at all → unverified.
        let (webui, _, _) = webui_evidence(None, false, 8021);
        assert_eq!(webui, "unverified");
    }

    #[test]
    fn error_lines_are_redacted_and_capped() {
        let mut text = String::from("dsh web: http://127.0.0.1:8021/?token=SECRETVALUE\n");
        text.push_str("ERROR: plugin dsh-foo has no create() factory\n");
        for i in 0..15 {
            text.push_str(&format!("error line {i}\n"));
        }
        let (_, _, errors) = webui_evidence(Some(&text), true, 8021);
        assert_eq!(errors.len(), MAX_ERROR_LINES);
        // The token must never leave the log excerpt…
        assert!(errors.iter().all(|l| !l.contains("SECRETVALUE")));
        // …and the known DSH plugin-load failure signature is caught.
        assert!(errors.iter().any(|l| l.contains("has no create()")));
        assert!(!errors.iter().any(|l| l.contains("dsh web:")));
    }

    #[test]
    fn overall_table() {
        assert_eq!(overall_of("pass", &[]), "pass");
        assert_eq!(overall_of("partial", &[]), "partial");
        assert_eq!(overall_of("unverified", &[]), "unverified");
        // An error line outweighs a green WebUI: 启动成功 ≠ 插件全部可用.
        assert_eq!(overall_of("pass", &["error".into()]), "fail");
    }

    #[tokio::test]
    async fn build_report_persists_an_honest_page() {
        let root = std::env::temp_dir().join(format!(
            "phl-trial-report-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dest = root.join("instances").join("t-rep1");
        tokio::fs::create_dir_all(dest.join("profiles").join("web"))
            .await
            .unwrap();
        write_manifest(&dest, &manifest("t-rep1", 1)).await.unwrap();
        super::super::marker::write_trial_marker(&dest, &marker("plan-rep1", "config"))
            .await
            .unwrap();

        // The persisted plan names the source — the report must carry it.
        super::super::plan::write_trial_plan(
            &root,
            &super::super::plan::TrialPlan {
                plan_id: "plan-rep1".into(),
                source_id: "daily-1".into(),
                target_id: "t-rep1".into(),
                target_version: "0.2.1-rc.5".into(),
                target_runtime_id: "node-system".into(),
                scope: "config".into(),
                workspace: "fresh".into(),
                source_fingerprint: "fp".into(),
                allocated_port: 1,
                auto_port: false,
                created_at: "now".into(),
            },
        )
        .await
        .unwrap();

        // One boot that printed a token URL and a plugin failure.
        let logs = dest.join("logs");
        tokio::fs::create_dir_all(&logs).await.unwrap();
        tokio::fs::write(
            logs.join("launch-2026-10-31T00-00-00.log"),
            "dsh web: http://127.0.0.1:1/?token=SECRETTOKEN\nERROR: plugin dsh-foo has no create() factory\n",
        )
        .await
        .unwrap();

        let report = build_report(&root, &dest, "t-rep1", &marker("plan-rep1", "config"))
            .await
            .unwrap();
        assert_eq!(report.source_id.as_deref(), Some("daily-1"));
        assert_eq!(report.target_version, "0.2.1-rc.5");
        assert!(report.launched_ever);
        // Port 1 is refused on any real machine → honest "not running".
        assert_eq!(report.webui, "unverified", "{}", report.webui_detail);
        assert_eq!(
            report.overall, "fail",
            "an error line must outweigh the rest"
        );
        assert!(report
            .error_lines
            .iter()
            .any(|l| l.contains("has no create()")));
        assert!(report
            .error_lines
            .iter()
            .all(|l| !l.contains("SECRETTOKEN")));
        assert_eq!(report.link_failures, vec!["link-x".to_string()]);
        assert!(
            report.sessions.is_none(),
            "config-only copy carries no session summary"
        );
        // The one-pager landed inside the copy's own directory and parses back.
        let persisted = report.report_path.as_ref().expect("persisted path");
        assert!(Path::new(persisted).exists(), "{persisted}");
        let round: serde_json::Value =
            serde_json::from_str(&tokio::fs::read_to_string(persisted).await.unwrap()).unwrap();
        assert_eq!(round["overall"], "fail");
        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    #[tokio::test]
    async fn sessions_copies_get_a_session_summary() {
        let root = std::env::temp_dir().join(format!(
            "phl-trial-report-s-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dest = root.join("instances").join("t-rep2");
        tokio::fs::create_dir_all(dest.join("profiles").join("web"))
            .await
            .unwrap();
        write_manifest(&dest, &manifest("t-rep2", 1)).await.unwrap();
        super::super::marker::write_trial_marker(&dest, &marker("plan-rep2", "config+sessions"))
            .await
            .unwrap();
        let report = build_report(
            &root,
            &dest,
            "t-rep2",
            &marker("plan-rep2", "config+sessions"),
        )
        .await
        .unwrap();
        let sessions = report
            .sessions
            .expect("config+sessions copies get a summary");
        assert_eq!(sessions.count, 0);
        assert!(sessions.unreadable.is_empty());
        let _ = tokio::fs::remove_dir_all(&root).await;
    }
}

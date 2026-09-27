//! The trial copy's own record (`phl-trial.json`): which plan created it and
//! what was true at commit time.
//!
//! Two consumers depend on it. The idempotent retry reports the RECORDED
//! outcome instead of inventing a fresh success (R2-03); and the plan-identity
//! check refuses to treat a copy created by another plan as this plan's
//! result. The plan's own knobs (version, runtime, port, scope, workspace)
//! ride along so a retry can be verified against the copy's commit-time facts
//! rather than against re-derived live state (R3-02). Every field except the
//! plan id defaults: a marker written before this shape still reads, and the
//! instance manifest then carries the version/runtime/port.

use std::path::Path;

use super::TrialOutcome;
use crate::instances::{build_record, load_manifest, InstanceManifest};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TrialMarker {
    pub plan_id: String,
    #[serde(default)]
    pub target_version: String,
    #[serde(default)]
    pub target_runtime_id: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub allocated_port: u16,
    pub readiness: String,
    #[serde(default)]
    pub link_failures: Vec<String>,
    /// Real profile packages that differed from the target tree at commit
    /// time (see `mismatched_profile_packages`); defaults for markers written
    /// before the field existed.
    #[serde(default)]
    pub mismatched_packages: Vec<String>,
}

pub(crate) async fn write_trial_marker(dest: &Path, marker: &TrialMarker) -> Result<(), String> {
    write_trial_marker_bytes(
        dest,
        &serde_json::to_vec_pretty(marker).map_err(|e| e.to_string())?,
    )
}

fn write_trial_marker_bytes(dest: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = dest.join(".phl-trial.json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest.join("phl-trial.json")).map_err(|e| e.to_string())
}

pub(crate) async fn read_trial_marker(dest: &Path) -> Option<TrialMarker> {
    let raw = tokio::fs::read_to_string(dest.join("phl-trial.json"))
        .await
        .ok()?;
    serde_json::from_str(&raw).ok()
}

/// The outcome a committed copy reports for a retry (R2-03/R3-02): its
/// persisted record, not a reconstructed one. The manifest is read fresh so
/// the record reflects the copy as it is now.
pub(crate) async fn recorded_outcome(dest: &Path, id: &str) -> Result<TrialOutcome, String> {
    let manifest = load_manifest(dest, id).await?;
    Ok(recorded_outcome_with(dest, manifest).await)
}

/// The same, for callers already holding the copy's manifest.
pub(crate) async fn recorded_outcome_with(dest: &Path, manifest: InstanceManifest) -> TrialOutcome {
    let marker = read_trial_marker(dest).await;
    let (readiness, link_failures, mismatched_packages, mut notes) = match marker {
        Some(m) => (
            m.readiness,
            m.link_failures,
            m.mismatched_packages,
            Vec::new(),
        ),
        // The copy committed but its commit-time outcome never landed (a
        // crash between the rename and the marker write). Say what the
        // manifest can support rather than reporting a clean ready.
        None => (
            super::readiness_of(&manifest, &[]),
            Vec::new(),
            Vec::new(),
            vec!["副本的提交记录缺失，状态按当前清单判断；如需确认请重新预览".to_string()],
        ),
    };
    notes.push("该计划的副本已存在：本次为重复提交，未创建新实例".to_string());
    TrialOutcome {
        record: build_record(dest, manifest).await,
        committed: true,
        readiness,
        link_redirects: 0,
        link_failures,
        sessions_imported: 0,
        notes,
        mismatched_packages,
    }
}

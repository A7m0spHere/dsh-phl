//! Community-import readiness: the one place that decides whether an
//! instance's community-pack import is usable, and the record state every
//! consumer reads (CR-06/D10/D17, R3-03, R4-01).
//!
//! Split out of `instances/mod.rs` when the parent crossed its size budget:
//! the instance list, the detail record, verify, the launch gate and the
//! dependency retry all ask THIS module, so there is exactly one answer.

use std::path::Path;

use super::manifest::InstanceManifest;

/* --------------------------- import readiness --------------------------- */

/// Community-import state, decided in ONE place so the instance list, the
/// detail record, verify, launch and the dependency retry can never disagree
/// about whether an import is usable (CR-06/D10/D17, R3-03).
///
/// The old projection folded "this is not an import" and "the marker is
/// missing or torn" into a single `None`, so a damaged record silently lifted
/// the launch gate on an instance whose dependencies were never installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImportState {
    /// No import provenance and no marker — an ordinary instance.
    NonCommunity,
    /// The record says dependency preparation has not finished.
    Pending,
    /// The record says the planned dependencies are installed.
    Ready,
    /// Import provenance says a record must exist, yet it is missing,
    /// unreadable, unparsable, or carries an unrecognisable stage. Fail
    /// closed: the dependencies cannot be shown to be installed, and — unlike
    /// `Pending` — the record cannot be rebuilt locally, because the plan it
    /// carried is exactly what is gone (R4-01: the copy must say so instead
    /// of promising a retry that cannot deliver).
    Corrupt(String),
}

impl ImportState {
    /// What the instance record exposes over the wire; `None` = an ordinary
    /// instance with nothing to report.
    pub(crate) fn wire(&self) -> Option<&'static str> {
        match self {
            ImportState::NonCommunity => None,
            ImportState::Pending => Some("needsDependencies"),
            ImportState::Ready => Some("readyToLaunch"),
            ImportState::Corrupt(_) => Some("corruptImport"),
        }
    }

    /// The user-readable launch refusal for this state, or `None` when the
    /// instance may boot.
    pub(crate) fn launch_refusal(&self, name: &str) -> Option<String> {
        match self {
            ImportState::NonCommunity | ImportState::Ready => None,
            ImportState::Pending => Some(format!(
                "实例「{name}」的整合包依赖尚未安装完成，请先在实例详情重试依赖安装"
            )),
            ImportState::Corrupt(reason) => Some(format!(
                "实例「{name}」的社区整合包导入记录不可用（{reason}）：\
                 无法确认依赖是否装好，启动已拒绝；请在实例详情重试依赖安装以重建记录"
            )),
        }
    }

    /// Whether the dependency retry may be ATTEMPTED: anything with an import
    /// record is a retry target, and a damaged record still gets its honest
    /// refusal from the marker reader ("导入记录损坏，无法确认计划依赖集") rather
    /// than a gate error. An instance with no import history is not a target
    /// at all. This is NOT a promise that the retry can repair the state —
    /// only `Ready`/`Pending` records can be completed in place (R4-01).
    pub(crate) fn is_import_target(&self) -> bool {
        !matches!(self, ImportState::NonCommunity)
    }
}

/// The community-import state of `dir`, judged from its marker AND its own
/// persisted provenance: only an import is expected to own a marker, so a
/// `.phlpack` install (`pack-installed`) or a trial copy (`trial-copy`)
/// without one stays a normal instance instead of reading as damaged.
pub(crate) async fn read_import_state(
    dir: &Path,
    manifest: Option<&InstanceManifest>,
) -> ImportState {
    import_state_of(read_import_marker(dir).await, manifest)
}

/// The dependency failures the import record holds for the last attempt
/// (CR-06/R4-01): what the retry card shows after a reload, so a failure stays
/// visible instead of living only in a toast.
pub(crate) async fn read_import_failures(dir: &Path) -> Vec<String> {
    let Ok(raw) = tokio::fs::read_to_string(dir.join("phl-import.json")).await else {
        return Vec::new();
    };
    let Ok(marker) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    marker
        .get("dependenciesFailed")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether PHL's own records say this instance came from a community pack
/// import — the one provenance that writes `phl-import.json`.
pub(crate) fn has_import_provenance(manifest: &InstanceManifest) -> bool {
    manifest
        .adopted_from
        .as_ref()
        .is_some_and(|a| a.mode == "community-import")
}

/// What the marker file itself yielded, before provenance is taken into
/// account. `Absent` is deliberately distinct from `Corrupt`: only the
/// instance's provenance can say whether a missing file is normal.
enum MarkerRead {
    Absent,
    Stage(String),
    Corrupt(String),
}

async fn read_import_marker(dir: &Path) -> MarkerRead {
    match tokio::fs::read_to_string(dir.join("phl-import.json")).await {
        Ok(raw) => match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(value) => match value.get("stage").and_then(|s| s.as_str()) {
                Some(stage) => MarkerRead::Stage(stage.to_string()),
                None => MarkerRead::Corrupt("记录缺少 stage 字段".into()),
            },
            Err(e) => MarkerRead::Corrupt(format!("记录损坏: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => MarkerRead::Absent,
        Err(e) => MarkerRead::Corrupt(format!("记录不可读取: {e}")),
    }
}

fn import_state_of(marker: MarkerRead, manifest: Option<&InstanceManifest>) -> ImportState {
    match marker {
        MarkerRead::Stage(stage) => match stage.as_str() {
            "ready" => ImportState::Ready,
            "needsDependencies" => ImportState::Pending,
            other => ImportState::Corrupt(format!("stage 无法识别: {other}")),
        },
        MarkerRead::Corrupt(reason) => ImportState::Corrupt(reason),
        MarkerRead::Absent => match manifest {
            Some(m) if has_import_provenance(m) => ImportState::Corrupt("记录缺失".into()),
            _ => ImportState::NonCommunity,
        },
    }
}

//! The `DshCandidate` wire type and its dedup identity.
//!
//! A candidate is one DSH *environment* observed on this machine — anchored
//! on its DSH_HOME directory, the thing adoption and isolation actually work
//! with. The executable is attached information, not the identity: two PATH
//! shims into the same install describe one environment, and a home without
//! any reachable executable is still a real, adoptable environment.
//!
//! The id is derived from the canonicalized home path (see `candidate_id`),
//! so the same directory found through several scan sources keeps one id and
//! the frontend's selection never forks over a rescan.

use serde::Serialize;

/// Which scan surfaced this candidate — drives the UI's provenance line and
/// the trust the frontend places on `confidence` (a manual pick outranks a
/// heuristic PATH hit; the default `~/.dsh` is the sanctioned one).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CandidateSource {
    /// `$DSH_HOME` environment override.
    Env,
    /// The resolved default home (`~/.dsh`).
    DefaultHome,
    /// A DSH executable found on PATH (npm global install).
    Path,
    /// Chosen by the user through a dialog; never second-guessed.
    Manual,
}

/// The PHL instance that already manages this home, when `already_managed`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedInstance {
    pub id: String,
    pub name: String,
}

/// How much the inspected directory proved of itself. `Invalid` is carried
/// only by single-path inspections (a manual pick that missed); list scans
/// drop invalid roots rather than reporting them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Confidence {
    /// Has the two structural anchors (a settings file and a profiles tree).
    High,
    /// Looks DSH-ish (sessions/storages/one anchor) but incomplete.
    Medium,
    /// Existence only — adoption must re-verify before copying.
    Low,
    /// Not a DSH home: reject, and tell the user why.
    Invalid,
}

/// One discovered DSH environment. The name mirrors the development spec's
/// `DshCandidate` field list verbatim so the two documents stay checkable
/// against each other.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DshCandidate {
    /// Stable id: `dsh-<first8-of-sha256(canonical home path)>`.
    pub id: String,
    /// A human label that never leaks an absolute machine path when a
    /// symbolic one exists (`~/.dsh` for the default home).
    pub display_name: String,
    pub dsh_home: String,
    /// Absolute path to a DSH entry point (`dsh.cmd` shim or `lib/bin.js`),
    /// when one was located. Adoption does not require it — PHL can launch
    /// through its own managed version — but it names *that* install's code.
    pub executable_path: Option<String>,
    /// Version of the harness this environment boots, read from the profile
    /// bundles (`@deepseek-ai/dsh-base/package.json`), not guessed.
    pub detected_version: Option<String>,
    pub node_path: Option<String>,
    pub node_version: Option<String>,
    /// The profile whose artifacts were counted (PHL convention: `web`).
    pub profile: String,
    pub plugin_count: usize,
    /// Count of session artifacts under `<home>/sessions` — a filename scan,
    /// no log parsing (see the spike doc §8).
    pub session_count: usize,
    /// Environment size on disk (bytes). Not in the spec's list, but the
    /// adoption preview cannot promise a copy cost without it.
    pub size_bytes: u64,
    pub source: CandidateSource,
    pub confidence: Confidence,
    pub warnings: Vec<String>,
    /// True when this home is already an instance's `dsh-home` inside the
    /// PHL data root. Adoption must refuse those; the UI dims them.
    pub already_managed: bool,
    pub managed_instance: Option<ManagedInstance>,
}

/// Dedup identity: canonicalize what exists, then normalise separators and
/// casing so `C:\Users\x\.dsh`, `C:/Users/x/.DSH` and `\\?\C:\...` collapse
/// to one key. Paths that do not exist keep their lexical form — they came
/// from a scan and will be re-canonicalized if adoption ever sees them.
pub(crate) fn home_key(path: &std::path::Path) -> String {
    let real = std::fs::canonicalize(path)
        .map(|p| crate::paths::strip_verbatim(&p))
        .unwrap_or_else(|_| path.to_path_buf());
    let text = real.to_string_lossy().replace('\\', "/");
    if cfg!(windows) {
        text.to_ascii_lowercase()
    } else {
        text
    }
}

/// The stable candidate id for one home directory.
pub(crate) fn candidate_id(home_path: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let key = home_key(home_path);
    let digest = Sha256::digest(key.as_bytes());
    let mut hex = String::with_capacity(8);
    for byte in &digest[..4] {
        use std::fmt::Write;
        let _ = write!(hex, "{byte:02x}");
    }
    format!("dsh-{hex}")
}

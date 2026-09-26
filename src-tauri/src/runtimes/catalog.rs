//! The nodejs.org dist index: parse the listing lines, group by major and
//! derive LTS codenames.

use serde::Deserialize;

use super::{parse_semver, NodeRuntimeMeta, MIN_MAJOR};

/* ------------------------------- catalog ------------------------------- */

#[derive(Deserialize)]
pub(crate) struct DistEntry {
    pub(crate) version: String,
    /// The index spells non-LTS as the JSON literal `false`, which
    /// `Option<String>` alone would reject.
    #[serde(default, deserialize_with = "lts_codename")]
    pub(crate) lts: Option<String>,
}

pub(crate) fn lts_codename<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    let value: serde_json::Value = Deserialize::deserialize(d)?;
    Ok(value.as_str().map(str::to_string))
}

pub(crate) struct MajorLine {
    major: u64,
    version: String,
    codename: Option<String>,
    semver: semver::Version,
}

/// Collapses the per-release index into one entry per major: the newest
/// release in the line, its LTS status, and its codename. The id is the
/// *precise* install id (`node-<full semver>`) so installing a catalog row
/// binds the exact object — major-grouped display is the frontend's job.
pub(crate) fn group_catalog(entries: Vec<DistEntry>) -> Vec<NodeRuntimeMeta> {
    let mut lines: Vec<MajorLine> = Vec::new();
    for entry in entries {
        let version = entry.version.trim_start_matches('v').to_string();
        let Some(semver) = parse_semver(&version) else {
            continue;
        };
        let line = MajorLine {
            major: semver.major,
            version,
            codename: entry.lts,
            semver,
        };
        let major = line.major;
        match lines.iter_mut().find(|l| l.major == major) {
            Some(existing) if existing.semver < line.semver => *existing = line,
            Some(_) => {}
            None => lines.push(line),
        }
    }
    lines.retain(|l| l.major >= MIN_MAJOR);
    lines.sort_by(|a, b| b.major.cmp(&a.major).then_with(|| b.semver.cmp(&a.semver)));
    lines
        .into_iter()
        .enumerate()
        .filter(|(rank, l)| l.codename.is_some() || *rank < 2)
        .map(|(_, l)| NodeRuntimeMeta {
            id: format!("node-{}", l.version),
            major: l.major,
            version: l.version,
            lts: l.codename.is_some(),
            codename: l.codename,
        })
        .collect()
}

/// Every release of one major line, newest first — the "other patch versions"
/// expander. Unlike the collapsed catalog this is not filtered to LTS lines:
/// a user pinning an older patch of an installed major needs the full list.
pub(crate) fn collect_major_versions(entries: Vec<DistEntry>, major: u64) -> Vec<NodeRuntimeMeta> {
    let mut lines: Vec<MajorLine> = Vec::new();
    for entry in entries {
        let version = entry.version.trim_start_matches('v').to_string();
        let Some(semver) = parse_semver(&version) else {
            continue;
        };
        if semver.major != major {
            continue;
        }
        lines.push(MajorLine {
            major,
            version,
            codename: entry.lts,
            semver,
        });
    }
    lines.sort_by(|a, b| b.semver.cmp(&a.semver));
    lines
        .into_iter()
        .map(|l| NodeRuntimeMeta {
            id: format!("node-{}", l.version),
            major: l.major,
            version: l.version,
            lts: l.codename.is_some(),
            codename: l.codename,
        })
        .collect()
}

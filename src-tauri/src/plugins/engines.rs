//! `engines.dsh` compatibility semantics (Issue #10).
//!
//! Official DSH (0.2.1-alpha.4 起) made a plugin package's own `engines.dsh`
//! the PRIMARY compatibility claim: the community catalog's verified-version
//! lists only apply when the field is absent. PHL follows that priority and
//! refuses to guess — a range that cannot be parsed, a DSH binding that is
//! not a clean semver, or a missing field all render as "unknown", never as
//! "incompatible". PHL only surfaces the claim; the install decision stays
//! with the user (issue #10: 未经授权不修改插件、不自动停用).
//!
//! The range syntax is npm's, which is wider than what the `semver` crate
//! parses directly: `||` alternatives, hyphen ranges, bare partial versions
//! ("1.2"), `x` wildcards, and whitespace between AND comparators all need
//! expanding into plain comparator sets first — that expander lives in ONE
//! place (`versions::range`, shared with the version catalog's
//! `majors_from_engines`)。Prerelease gating follows
//! npm: a prerelease DSH version only satisfies a range that names a
//! prerelease of the same major.minor.patch tuple — the `semver` crate
//! implements that rule natively, so evaluation after normalization is
//! exactly npm's `satisfies`.

use std::path::Path;

use serde::Serialize;
use tauri::State;

/* ------------------------------ wire types ---------------------------- */

/// One installed plugin's `engines.dsh` claim evaluated against ONE concrete
/// DSH version (the instance's binding, or a trial's target version).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginEngineCompat {
    pub plugin_id: String,
    pub registry_id: String,
    pub installed_version: String,
    /// The raw `engines.dsh` string; `None` when the package does not
    /// declare one — the legacy catalog-list rule applies then (issue #10
    /// keeps both sources, engines.dsh first).
    pub engine_range: Option<String>,
    /// `compatible` | `incompatible` | `unknown`.
    pub verdict: String,
    pub reason: String,
}

/// Internal verdict; the wire copy carries it as a plain string so the
/// frontend union stays decoupled from Rust enum names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineVerdict {
    Compatible,
    Incompatible,
    Unknown,
}

impl EngineVerdict {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EngineVerdict::Compatible => "compatible",
            EngineVerdict::Incompatible => "incompatible",
            EngineVerdict::Unknown => "unknown",
        }
    }
}

/// The bare DSH version behind a manifest's canonical `dsh-<ver>` binding.
/// `None` for an empty/未绑定 binding or anything that is not clean semver —
/// which every caller must surface as "unknown", not as a guess.
pub(crate) fn bare_dsh_version(version_id: &str) -> Option<semver::Version> {
    let bare = version_id.strip_prefix("dsh-").unwrap_or(version_id);
    semver::Version::parse(bare.trim()).ok()
}

/// The `engines.dsh` string inside an already-parsed package.json, or `None`
/// when absent/empty/non-string.
pub(crate) fn engine_range_of(package_json: &serde_json::Value) -> Option<String> {
    let raw = package_json
        .get("engines")?
        .get("dsh")?
        .as_str()?
        .trim()
        .to_string();
    (!raw.is_empty()).then_some(raw)
}

/// Read `<pkg_dir>/package.json` and extract `engines.dsh`.
pub(crate) async fn package_engine_range(pkg_dir: &Path) -> Option<String> {
    let raw = tokio::fs::read_to_string(pkg_dir.join("package.json"))
        .await
        .ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    engine_range_of(&value)
}

/// Per-plugin verdicts for a list of installed plugins against one DSH
/// version. Read-only: nothing under `profile` is touched. The plugin list
/// arrives as `(plugin_id, registry_id, installed_version)` triples so this
/// core stays decoupled from `instances` types — the caller does the join
/// from whatever scanner it already runs.
pub(crate) async fn compat_for_installed(
    plugins: &[(String, String, String)],
    profile: &Path,
    dsh: Option<semver::Version>,
) -> Vec<PluginEngineCompat> {
    let mut out = Vec::with_capacity(plugins.len());
    for (plugin_id, registry_id, version) in plugins {
        let pkg_dir = profile.join("node_modules").join(registry_id);
        let range = package_engine_range(&pkg_dir).await;
        let (verdict, reason) = evaluate(range.as_deref(), dsh.as_ref());
        out.push(PluginEngineCompat {
            plugin_id: plugin_id.clone(),
            registry_id: registry_id.clone(),
            installed_version: version.clone(),
            engine_range: range,
            verdict: verdict.as_str().to_string(),
            reason,
        });
    }
    out
}

/// Evaluate one `engines.dsh` string against one DSH version with npm
/// semantics. Returns `(verdict, human-readable reason)`.
pub(crate) fn evaluate(
    range: Option<&str>,
    dsh: Option<&semver::Version>,
) -> (EngineVerdict, String) {
    let Some(dsh) = dsh else {
        return (
            EngineVerdict::Unknown,
            "实例未绑定可解析的 DSH 版本，无法核对 engines.dsh".into(),
        );
    };
    let Some(range) = range.map(str::trim).filter(|r| !r.is_empty()) else {
        return (
            EngineVerdict::Unknown,
            "插件未声明 engines.dsh，按官方规则沿用兼容性记录判定".into(),
        );
    };
    let Some(reqs) = crate::versions::parse_npm_range(range) else {
        return (
            EngineVerdict::Unknown,
            format!("engines.dsh 值「{range}」无法按 npm 语义解析，不做兼容性判断"),
        );
    };
    if reqs.iter().any(|req| req.matches(dsh)) {
        (
            EngineVerdict::Compatible,
            format!("engines.dsh 声明 {range}，当前 DSH {dsh} 在声明范围内"),
        )
    } else {
        let mut reason = format!("engines.dsh 声明 {range}，当前 DSH {dsh} 不在声明范围内");
        if !dsh.pre.is_empty() {
            reason.push_str("（npm 语义：预发布版本只在范围显式包含同一版本的预发布时匹配）");
        }
        (EngineVerdict::Incompatible, reason)
    }
}

/* --------------------- npm range parsing (shared) --------------------- */

// The npm syntax expander lives in ONE place: versions::range — the plugin
// engines check and the version catalog (majors_from_engines) must never
// drift apart on range semantics.

/// Command: read-only per-plugin `engines.dsh` verdicts for one instance's
/// installed plugins, evaluated against the instance's bound DSH version.
/// External (in-place adopted) instances stay read-only — reads are never
/// gated, and nothing here writes.
#[tauri::command]
pub async fn installed_plugin_compat(
    phl: State<'_, crate::paths::PhlState>,
    instance_id: String,
) -> Result<Vec<PluginEngineCompat>, String> {
    let root = phl.root();
    let dir = crate::instances::instance_dir(&root, &instance_id)?;
    let manifest = crate::instances::manifest::load_manifest(&dir, &instance_id).await?;
    let dsh = bare_dsh_version(&manifest.version_id);
    let profile = crate::instances::profile_dir(&root, &instance_id).await?;
    let scanned = crate::instances::scan_plugins(&profile).await;
    let triples: Vec<(String, String, String)> = scanned
        .into_iter()
        .map(|p| (p.plugin_id, p.registry_id, p.version))
        .collect();
    Ok(compat_for_installed(&triples, &profile, dsh).await)
}

/* ------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use EngineVerdict::{Compatible, Incompatible, Unknown};

    fn v(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    #[test]
    fn parse_and_evaluate_table() {
        // (engines.dsh, DSH candidate, expected verdict)
        let cases: &[(&str, &str, EngineVerdict)] = &[
            ("^1.2.3", "1.2.3", Compatible),
            ("^1.2.3", "1.3.0", Compatible),
            ("^1.2.3", "2.0.0", Incompatible),
            ("~1.2.3", "1.2.9", Compatible),
            ("~1.2.3", "1.3.0", Incompatible),
            // npm: bare full version is EXACT, not caret
            ("1.2.3", "1.2.3", Compatible),
            ("1.2.3", "1.2.4", Incompatible),
            (">= 1.0.0", "1.5.0", Compatible), // whitespace after the op
            ("1.x", "1.9.9", Compatible),
            ("1.x", "2.0.0", Incompatible),
            ("1.2.x", "1.2.9", Compatible),
            ("1.2.x", "1.3.0", Incompatible),
            ("*", "1.2.3", Compatible),
            ("x", "9.9.9", Compatible),
            (">=1.0.0 <2.0.0", "1.5.0", Compatible),
            (">=1.0.0 <2.0.0", "2.0.0", Incompatible),
            ("1.2.3 - 1.5.0", "1.3.0", Compatible),
            ("1.2.3 - 1.5.0", "1.6.0", Incompatible),
            ("1.2.3 - 2.3", "2.3.9", Compatible),
            ("1.2.3 - 2.3", "2.4.0", Incompatible),
            ("1.2.3 - 2", "2.9.9", Compatible),
            ("1.2.3 - 2", "3.0.0", Incompatible),
            ("v1.2.3", "1.2.3", Compatible),
            ("=1.2.3", "1.2.3", Compatible),
            (">=0.1.0 || >=0.2.0", "0.1.5", Compatible),
            (">=0.1.0 || >=0.2.0", "0.2.5", Compatible),
            ("^0.1.0 || ^0.2.0", "0.3.0", Incompatible),
            (">1.2", "1.2.9", Incompatible),
            (">1.2", "1.3.0", Compatible),
            (">=1.2", "1.2.0", Compatible),
            ("<=1.2", "1.2.9", Compatible),
            ("<1.2", "1.1.9", Compatible),
            ("<1.2", "1.2.0", Incompatible),
            ("~>1.2", "1.2.9", Compatible),
            // npm prerelease gating: a prerelease DSH only matches a range
            // that names a prerelease of the same major.minor.patch
            ("^0.1.0", "0.1.5-rc.3", Incompatible),
            (">=0.1.5-rc.1", "0.1.5-rc.3", Compatible),
            ("*", "0.2.1-rc.2", Incompatible),
            ("*", "0.2.1", Compatible),
            ("0.1.x", "0.1.5-rc.3", Incompatible),
            // unparseable → unknown, never "incompatible"
            ("garbage", "1.0.0", Unknown),
            ("1.2.3.4", "1.0.0", Unknown),
            (">=1.0.0 <garbage", "1.0.2", Unknown),
            ("1.2-rc.1", "1.2.0", Unknown), // prerelease on a partial
        ];
        for (range, candidate, expected) in cases {
            let dsh = v(candidate);
            let (verdict, reason) = evaluate(Some(*range), Some(&dsh));
            assert_eq!(
                verdict, *expected,
                "range {range:?} vs {candidate}: {reason}"
            );
        }
    }

    #[test]
    fn empty_and_missing_ranges_are_unknown() {
        let dsh = v("0.1.5");
        assert_eq!(evaluate(None, Some(&dsh)).0, Unknown);
        assert_eq!(evaluate(Some(""), Some(&dsh)).0, Unknown);
        assert_eq!(evaluate(Some("   "), Some(&dsh)).0, Unknown);
        assert_eq!(evaluate(Some("*"), None).0, Unknown);
    }

    #[test]
    fn same_plugin_two_dsh_versions_diverge() {
        // Issue #10 acceptance: the SAME engines.dsh must yield DIFFERENT,
        // evidence-backed verdicts on two DSH versions.
        let range = Some("^0.1.0");
        let (old_verdict, old_reason) = evaluate(range, Some(&v("0.1.5")));
        let (new_verdict, new_reason) = evaluate(range, Some(&v("0.2.0")));
        assert_eq!(old_verdict, Compatible);
        assert_eq!(new_verdict, Incompatible);
        assert!(old_reason.contains("^0.1.0"), "{old_reason}");
        assert!(new_reason.contains("0.2.0"), "{new_reason}");
    }

    #[test]
    fn bare_dsh_version_strips_the_binding_prefix() {
        assert_eq!(
            bare_dsh_version("dsh-0.1.5-rc.1").unwrap().to_string(),
            "0.1.5-rc.1"
        );
        assert_eq!(
            bare_dsh_version("0.2.1-rc.2").unwrap().to_string(),
            "0.2.1-rc.2"
        );
        assert!(bare_dsh_version("").is_none());
        assert!(bare_dsh_version("dsh-").is_none());
        assert!(bare_dsh_version("dsh-not-a-version").is_none());
    }

    #[test]
    fn engine_range_of_table() {
        let mk = |json: &str| serde_json::from_str::<serde_json::Value>(json).unwrap();
        let read = |json: &str| engine_range_of(&mk(json));
        assert_eq!(
            read(r#"{"engines":{"dsh":">=0.1.0"}}"#).as_deref(),
            Some(">=0.1.0")
        );
        // whitespace is normalized away at extraction
        assert_eq!(
            read(r#"{"engines":{"dsh":" ^0.1.0 "}}"#).as_deref(),
            Some("^0.1.0")
        );
        assert_eq!(read(r#"{"engines":{"node":">=20"}}"#).as_deref(), None);
        assert_eq!(read(r#"{"engines":{"dsh":42}}"#).as_deref(), None);
        assert_eq!(read(r#"{"engines":{"dsh":"  "}}"#).as_deref(), None);
        assert_eq!(read(r#"{"engines":{}}"#).as_deref(), None);
        assert_eq!(read("{}").as_deref(), None);
    }

    #[tokio::test]
    async fn compat_for_installed_reads_package_json_and_never_guesses() {
        let temp = std::env::temp_dir().join(format!(
            "phl-engines-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let profile = temp.join("profiles").join("web");
        let pkg = profile.join("node_modules").join("dsh-foo");
        tokio::fs::create_dir_all(&pkg).await.unwrap();
        tokio::fs::write(
            pkg.join("package.json"),
            r#"{"name":"dsh-foo","version":"1.4.0","engines":{"dsh":"^0.1.0"}}"#,
        )
        .await
        .unwrap();
        let plugins = vec![(
            "dsh-foo".to_string(),
            "dsh-foo".to_string(),
            "1.4.0".to_string(),
        )];
        let compat = compat_for_installed(&plugins, &profile, Some(v("0.1.5"))).await;
        assert_eq!(compat.len(), 1);
        assert_eq!(compat[0].verdict, "compatible");
        assert_eq!(compat[0].engine_range.as_deref(), Some("^0.1.0"));
        assert!(
            compat[0].reason.contains("在声明范围内"),
            "{}",
            compat[0].reason
        );

        // no package.json → range None → "unknown", never a guess
        tokio::fs::remove_file(pkg.join("package.json"))
            .await
            .unwrap();
        let compat = compat_for_installed(&plugins, &profile, Some(v("0.1.5"))).await;
        assert_eq!(compat[0].verdict, "unknown");

        // unbound DSH version → "unknown" for every plugin
        tokio::fs::write(pkg.join("package.json"), r#"{"engines":{"dsh":"^0.1.0"}}"#)
            .await
            .unwrap();
        let compat = compat_for_installed(&plugins, &profile, None).await;
        assert_eq!(compat[0].verdict, "unknown");

        let _ = tokio::fs::remove_dir_all(&temp).await;
    }
}

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
//! expanding into plain comparator sets first (`versions` 的
//! `majors_from_engines` 出于同样原因手工拆 `||`)。Prerelease gating follows
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

/// Per-plugin verdicts for a scanned plugin list against one DSH version.
/// Read-only: nothing under `profile` is touched.
pub(crate) async fn compat_for_installed(
    plugins: &[crate::instances::InstalledPluginInfo],
    profile: &Path,
    dsh: Option<semver::Version>,
) -> Vec<PluginEngineCompat> {
    let mut out = Vec::with_capacity(plugins.len());
    for plugin in plugins {
        let pkg_dir = profile.join("node_modules").join(&plugin.registry_id);
        let range = package_engine_range(&pkg_dir).await;
        let (verdict, reason) = evaluate(range.as_deref(), dsh.as_ref());
        out.push(PluginEngineCompat {
            plugin_id: plugin.plugin_id.clone(),
            registry_id: plugin.registry_id.clone(),
            installed_version: plugin.version.clone(),
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
    let Some(reqs) = parse_dsh_range(range) else {
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

/* --------------------------- npm range parsing ------------------------ */

/// Parse an npm-style range into the comparator sets the `semver` crate can
/// evaluate. `None` = unparseable (the caller must report "unknown").
///
/// Split on `||` first (the crate only understands `,`-separated AND
/// comparators — the same constraint `versions::majors_from_engines` works
/// around), then expand each alternative's npm-specific syntax: hyphen
/// ranges, bare partials, `x` wildcards, `v` prefixes and whitespace between
/// comparators.
pub(crate) fn parse_dsh_range(range: &str) -> Option<Vec<semver::VersionReq>> {
    let range = range.trim();
    if range.is_empty() {
        return None;
    }
    let mut reqs = Vec::new();
    for alt in range.split("||") {
        let comparators = expand_alternative(alt)?;
        let text = if comparators.is_empty() {
            "*".to_string()
        } else {
            comparators.join(", ")
        };
        reqs.push(semver::VersionReq::parse(&text).ok()?);
    }
    Some(reqs)
}

fn expand_alternative(alt: &str) -> Option<Vec<String>> {
    // npm allows whitespace between an operator and its operand (">= 1.0.0").
    let raw: Vec<&str> = alt.split_whitespace().filter(|t| !t.is_empty()).collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let t = raw[i];
        if matches!(t, ">=" | "<=" | ">" | "<" | "=" | "^" | "~" | "~>") && i + 1 < raw.len() {
            tokens.push(format!("{t}{}", raw[i + 1]));
            i += 2;
        } else {
            tokens.push(t.to_string());
            i += 1;
        }
    }
    // Hyphen ranges need the RAW operands on both sides, so resolve them in
    // a dedicated pass before per-token expansion; the resolved list acts as
    // a stack so `A - B` pops the already-seen `A`.
    let mut resolved: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < tokens.len() {
        if tokens[idx] == "-" {
            if resolved.is_empty() || idx + 1 >= tokens.len() {
                return None;
            }
            let lo = resolved.pop()?;
            let hi = tokens[idx + 1].clone();
            if !is_operand(&lo) || !is_operand(&hi) {
                return None;
            }
            resolved.extend(hyphen_range(&lo, &hi)?);
            idx += 2;
        } else {
            resolved.push(tokens[idx].clone());
            idx += 1;
        }
    }
    let mut out: Vec<String> = Vec::new();
    for token in &resolved {
        out.extend(expand_token(token)?);
    }
    Some(out)
}

fn is_operand(token: &str) -> bool {
    !token.is_empty()
        && !token.starts_with(['>', '<', '=', '^', '~'])
        && !matches!(token, "-" | "*" | "x" | "X")
}

fn hyphen_range(lo: &str, hi: &str) -> Option<Vec<String>> {
    let lower = match parse_operand(lo.trim().trim_start_matches('v'))? {
        Operand::Full(v) => format!(">={v}"),
        Operand::Partial { major, minor } => format!(">={major}.{}.0", minor.unwrap_or(0)),
    };
    // npm's own desugar: a partial upper bound excludes the next segment,
    // with `-0` so prereleases of the boundary itself stay comparable.
    let upper = match parse_operand(hi.trim().trim_start_matches('v'))? {
        Operand::Full(v) => format!("<={v}"),
        Operand::Partial { major, minor } => match minor {
            Some(m) => format!("<{major}.{}.0-0", m + 1),
            None => format!("<{}.0.0-0", major + 1),
        },
    };
    Some(vec![lower, upper])
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Bare,
    Eq,
    Ge,
    Le,
    Gt,
    Lt,
    Caret,
    Tilde,
}

fn expand_token(token: &str) -> Option<Vec<String>> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let (op, operand) = if let Some(rest) = token.strip_prefix(">=") {
        (Op::Ge, rest)
    } else if let Some(rest) = token.strip_prefix("<=") {
        (Op::Le, rest)
    } else if let Some(rest) = token.strip_prefix("~>") {
        (Op::Tilde, rest) // legacy npm alias for `~`
    } else if let Some(rest) = token.strip_prefix('^') {
        (Op::Caret, rest)
    } else if let Some(rest) = token.strip_prefix('~') {
        (Op::Tilde, rest)
    } else if let Some(rest) = token.strip_prefix('>') {
        (Op::Gt, rest)
    } else if let Some(rest) = token.strip_prefix('<') {
        (Op::Lt, rest)
    } else if let Some(rest) = token.strip_prefix('=') {
        (Op::Eq, rest)
    } else {
        (Op::Bare, token)
    };
    let operand = operand.trim().trim_start_matches('v');
    if operand.is_empty() {
        return None;
    }
    if matches!(operand, "*" | "x" | "X") {
        // npm: a wildcard operand matches everything, any operator included.
        return Some(Vec::new());
    }
    let spec = parse_operand(operand)?;
    match spec {
        Operand::Full(v) => Some(vec![full_comparator(op, &v)]),
        Operand::Partial { major, minor } => partial_comparators(op, major, minor),
    }
}

fn full_comparator(op: Op, v: &semver::Version) -> String {
    match op {
        Op::Bare | Op::Eq => format!("={v}"),
        Op::Ge => format!(">={v}"),
        Op::Le => format!("<={v}"),
        Op::Gt => format!(">{v}"),
        Op::Lt => format!("<{v}"),
        Op::Caret => format!("^{v}"),
        Op::Tilde => format!("~{v}"),
    }
}

fn partial_comparators(op: Op, major: u64, minor: Option<u64>) -> Option<Vec<String>> {
    match op {
        Op::Bare | Op::Eq => {
            // npm: "1.2" := ">=1.2.0 <1.3.0-0"; "1" := ">=1.0.0 <2.0.0-0"
            Some(match minor {
                Some(m) => vec![
                    format!(">={major}.{m}.0"),
                    format!("<{major}.{}.0-0", m + 1),
                ],
                None => vec![format!(">={major}.0.0"), format!("<{}.0.0-0", major + 1)],
            })
        }
        Op::Caret => {
            // npm: ^1.2 := >=1.2.0 <2.0.0-0; ^0.2 := >=0.2.0 <0.3.0-0;
            // ^0.0 := >=0.0.0 <0.1.0-0
            let upper = match (major, minor) {
                (0, Some(m)) => format!("0.{}.0-0", m + 1),
                _ => format!("{}.0.0-0", major + 1),
            };
            let lower = format!(">={major}.{}.0", minor.unwrap_or(0));
            Some(vec![lower, format!("<{upper}")])
        }
        Op::Tilde => {
            // npm: ~1.2 := >=1.2.0 <1.3.0-0; ~1 := >=1.0.0 <2.0.0-0
            match minor {
                Some(m) => Some(vec![
                    format!(">={major}.{m}.0"),
                    format!("<{major}.{}.0-0", m + 1),
                ]),
                None => Some(vec![
                    format!(">={major}.0.0"),
                    format!("<{}.0.0-0", major + 1),
                ]),
            }
        }
        Op::Ge => Some(vec![format!(">={major}.{}.0", minor.unwrap_or(0))]),
        // npm: >1.2 := >=1.3.0; >1 := >=2.0.0
        Op::Gt => Some(vec![match minor {
            Some(m) => format!(">={major}.{}.0", m + 1),
            None => format!(">={}.0.0", major + 1),
        }]),
        Op::Lt => Some(vec![match minor {
            // npm: <1.2 := <1.2.0-0; <1 := <1.0.0-0
            Some(m) => format!("<{major}.{m}.0-0"),
            None => format!("<{major}.0.0-0"),
        }]),
        Op::Le => Some(vec![match minor {
            // npm: <=1.2 := <1.3.0-0; <=1 := <2.0.0-0
            Some(m) => format!("<{major}.{}.0-0", m + 1),
            None => format!("<{}.0.0-0", major + 1),
        }]),
    }
}

/// npm-range operand: a full semver (prerelease allowed) or a numeric prefix
/// with optional wildcard segments.
enum Operand {
    Full(semver::Version),
    Partial { major: u64, minor: Option<u64> },
}

fn parse_operand(s: &str) -> Option<Operand> {
    // build metadata never appears in npm ranges — refuse, don't guess
    if s.contains('+') {
        return None;
    }
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (s, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut numeric: Vec<Option<u64>> = Vec::with_capacity(parts.len());
    for part in &parts {
        if matches!(*part, "x" | "X" | "*") {
            numeric.push(None);
        } else if !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) {
            numeric.push(part.parse().ok());
        } else {
            return None;
        }
    }
    // a wildcard cannot be followed by a concrete segment ("1.x.3" is nonsense)
    if let Some(pos) = numeric.iter().position(|n| n.is_none()) {
        if numeric[pos..].iter().any(|n| n.is_some()) {
            return None;
        }
    }
    let major = numeric.first().copied().flatten()?;
    // The wildcard check above guarantees every None is trailing, so the
    // concrete prefix length decides full vs partial ("1.2.x" is a partial).
    let concrete = numeric
        .iter()
        .position(|n| n.is_none())
        .unwrap_or(numeric.len());
    // npm rejects a prerelease on a partial ("1.2-rc.1" without patch) —
    // unknown, not a guess
    if concrete < 3 && pre.is_some() {
        return None;
    }
    if concrete == 3 {
        // A full version needs all three segments concrete.
        let minor = numeric[1]?;
        let patch = numeric[2]?;
        let text = match pre {
            Some(p) => format!("{major}.{minor}.{patch}-{p}"),
            None => format!("{major}.{minor}.{patch}"),
        };
        let v = semver::Version::parse(&text).ok()?;
        Some(Operand::Full(v))
    } else {
        Some(Operand::Partial {
            major,
            minor: if concrete >= 2 { numeric[1] } else { None },
        })
    }
}

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
    let plugins = crate::instances::scan_plugins(&profile).await;
    Ok(compat_for_installed(&plugins, &profile, dsh).await)
}

/* ------------------------------- tests -------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::InstalledPluginInfo;
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
        let plugins = vec![InstalledPluginInfo {
            plugin_id: "dsh-foo".into(),
            registry_id: "dsh-foo".into(),
            version: "1.4.0".into(),
            enabled: true,
            trust: "verified".into(),
            linked: false,
            manageable: true,
            display_name: None,
        }];
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

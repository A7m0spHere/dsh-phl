//! npm range syntax → `semver` crate comparator sets, in ONE place.
//!
//! The `semver` crate only understands `,`-separated AND comparators; npm's
//! `engines`-style ranges are wider: `||` alternatives, hyphen ranges, bare
//! partial versions ("1.2"), `x`/`X` wildcards, `v` prefixes and whitespace
//! between AND comparators. Every consumer that used to hand-roll part of
//! this (`majors_from_engines` split `||` manually; a partial like
//! `">=18.17"` simply failed to parse) now expands through this module
//! first, so the semantics live here and cannot drift apart.

/// Parse an npm-style range into the comparator sets the `semver` crate can
/// evaluate. `None` = unparseable (the caller must report "unknown" —
/// exactly npm's own behaviour for a range it cannot read).
pub(crate) fn parse_npm_range(range: &str) -> Option<Vec<semver::VersionReq>> {
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

#[cfg(test)]
mod tests {
    use super::parse_npm_range;

    fn reqs(range: &str) -> Vec<semver::VersionReq> {
        parse_npm_range(range).unwrap_or_else(|| panic!("{range} must parse"))
    }

    fn matches(range: &str, version: &str) -> bool {
        let v = semver::Version::parse(version).unwrap();
        reqs(range).iter().any(|req| req.matches(&v))
    }

    #[test]
    fn npm_syntax_expands_to_the_same_semantics_npm_has() {
        assert!(matches("^1.2.3", "1.3.0"));
        assert!(!matches("^1.2.3", "2.0.0"));
        // bare full version = EXACT in npm, not caret
        assert!(matches("1.2.3", "1.2.3"));
        assert!(!matches("1.2.3", "1.2.4"));
        // partials and wildcards
        assert!(matches("1.x", "1.9.9"));
        assert!(!matches("1.x", "2.0.0"));
        assert!(matches("1.2", "1.2.9"));
        assert!(!matches("1.2", "1.3.0"));
        assert!(matches("*", "9.9.9"));
        // hyphen ranges, `v` prefix, `||`, whitespace after operators
        assert!(matches("1.2.3 - 2.3", "2.3.9"));
        assert!(!matches("1.2.3 - 2.3", "2.4.0"));
        assert!(matches("v1.2.3", "1.2.3"));
        assert!(matches(">=0.1.0 || >=0.2.0", "0.2.5"));
        assert!(matches(">= 1.0.0", "1.5.0"));
        // the partial-with-operator form `majors_from_engines` used to drop
        assert!(matches(">=18.17", "18.19.0"));
        assert!(!matches(">=18.17", "18.16.0"));
    }

    #[test]
    fn unparseable_ranges_are_none() {
        assert!(parse_npm_range("").is_none());
        assert!(parse_npm_range("garbage").is_none());
        assert!(parse_npm_range("1.2.3.4").is_none());
        assert!(parse_npm_range("1.2-rc.1").is_none());
        assert!(parse_npm_range(">=1.0.0 <garbage").is_none());
    }
}

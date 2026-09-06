//! How instance environment variables are classified when they cross a
//! shareable boundary — today that is the Bundle (export and import).
//!
//! Two independent signals decide whether a variable's *value* may travel:
//!
//! * **Explicit** — the name is a credential carrier by construction. Every
//!   provider in the global library declares `apiKeyEnv`, and launch injects
//!   the real key under exactly that name; PHL itself therefore knows these
//!   names hold secrets, no guessing involved. This signal wins: an explicit
//!   name is stripped even when the heuristic would miss it.
//! * **Name heuristic** — the name contains a credential word as a whole
//!   segment (`OPENAI_API_KEY`, `GITHUB_TOKEN`). This only *supplements* the
//!   explicit set, because classification happens on names, not values: a key
//!   stored under an innocuous name (`HTTP_PROXY`, `LANG`) is not detected.
//!   That residual risk is a documented boundary, not something to paper over
//!   with a wider blacklist.
//!
//! The remaining category is machine-local state: variables that redirect
//! execution or escape the instance's isolation (`NODE_OPTIONS`, `PATH`,
//! `DSH_HOME`, …). Their values never travel either — import drops them, so
//! exporting them would only promise what import then breaks.

use std::collections::{HashMap, HashSet};

/// Environment variables whose value executes code or breaks the DSH_HOME
/// isolation, and so must never survive an import. Shared with the export
/// classifier: shipping a value that import then drops would be misleading.
///
/// A bundle is the format PHL tells users to share, so its contents are
/// attacker-supplied by design. `run_launch` applies instance env verbatim
/// (minus `DSH_HOME`), which means an imported `NODE_OPTIONS=--require
/// C:\evil.js` would run on the first 启动. Filtering belongs here, at the
/// trust boundary, rather than in the launcher's own allow-list.
const UNSAFE_IMPORT_ENV: &[&str] = &[
    "NODE_OPTIONS",
    "NODE_REPL_EXTERNAL_MODULE",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "PATH",
    "NODE_PATH",
];

/// The outcome of partitioning an env map at a shareable boundary.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct EnvPartition {
    /// Variables whose values may travel as-is.
    pub env: HashMap<String, String>,
    /// Names whose values were stripped because they are credential
    /// carriers. The importer surfaces these as "needs re-configuration".
    pub credentials: Vec<String>,
    /// Names whose values were stripped as machine-local state — not
    /// credentials, but code-injection vectors or the isolation boundary.
    pub machine_only: Vec<String>,
}

fn upper(key: &str) -> String {
    key.to_ascii_uppercase()
}

/// Whole-segment credential words. Matching segments (not substrings) keeps
/// `MONKEY` from reading as a key while still catching every conventional
/// credential name shape.
fn name_suggests_credential(name: &str) -> bool {
    upper(name)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|segment| {
            matches!(
                segment,
                "KEY"
                    | "KEYS"
                    | "TOKEN"
                    | "TOKENS"
                    | "SECRET"
                    | "SECRETS"
                    | "PASS"
                    | "PASSWORD"
                    | "PASSWD"
                    | "PASSPHRASE"
                    | "CREDENTIAL"
                    | "CREDENTIALS"
                    | "AUTH"
                    | "AUTHORIZATION"
                    | "APIKEY"
                    | "APIKEYS"
            )
        })
}

/// Partitions `env` into shareable values, stripped credential carriers and
/// stripped machine-local state. Explicit names take priority: a name known
/// from the library is stripped even when the heuristic misses it.
pub(crate) fn partition_env(
    env: HashMap<String, String>,
    credential_envs: &HashSet<String>,
) -> EnvPartition {
    let mut out = EnvPartition::default();
    for (key, value) in env {
        let name = upper(&key);
        if name == "DSH_HOME" || UNSAFE_IMPORT_ENV.contains(&name.as_str()) {
            out.machine_only.push(key);
        } else if credential_envs.contains(&name) || name_suggests_credential(&key) {
            out.credentials.push(key);
        } else {
            out.env.insert(key, value);
        }
    }
    out.credentials.sort();
    out.machine_only.sort();
    out
}

/// The import-side view: machine-local values are dropped silently (as the
/// old `sanitize_imported_env` did), credential *values* are dropped and
/// their names reported so the UI can ask for re-configuration.
pub(crate) fn partition_imported_env(
    env: HashMap<String, String>,
    credential_envs: &HashSet<String>,
) -> (HashMap<String, String>, Vec<String>) {
    let partition = partition_env(env, credential_envs);
    (partition.env, partition.credentials)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn heuristic_matches_conventional_credential_names_only() {
        for name in [
            "OPENAI_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "MY_PASSWORD",
            "DB_CREDENTIALS",
            "ANTHROPIC_AUTH_TOKEN",
            "apikey", // no separator at all
        ] {
            assert!(name_suggests_credential(name), "{name} should match");
        }
        for name in ["HTTP_PROXY", "NO_PROXY", "LANG", "TZ", "MONKEY_PATCH"] {
            assert!(!name_suggests_credential(name), "{name} should not match");
        }
    }

    #[test]
    fn explicit_names_win_over_the_heuristic() {
        // Not a credential word by the heuristic…
        assert!(!name_suggests_credential("MY_GATE_SLOT"));
        // …but the library says this exact name carries a key.
        let (kept, credentials) = partition_imported_env(
            HashMap::from([("MY_GATE_SLOT".into(), "value".into())]),
            &set(&["MY_GATE_SLOT"]),
        );
        assert!(kept.is_empty());
        assert_eq!(credentials, vec!["MY_GATE_SLOT".to_string()]);
    }

    #[test]
    fn machine_local_names_never_travel_and_are_not_credentials() {
        let partition = partition_env(
            HashMap::from([
                ("DSH_HOME".into(), "C:\\elsewhere".into()),
                ("PATH".into(), "C:\\bin".into()),
                ("NODE_OPTIONS".into(), "--require x.js".into()),
            ]),
            &HashSet::new(),
        );
        assert!(partition.env.is_empty());
        assert_eq!(
            partition.machine_only,
            vec![
                "DSH_HOME".to_string(),
                "NODE_OPTIONS".to_string(),
                "PATH".to_string()
            ]
        );
        assert!(partition.credentials.is_empty());
    }

    #[test]
    fn ordinary_variables_keep_their_values() {
        let (kept, credentials) = partition_imported_env(
            HashMap::from([
                ("HTTP_PROXY".into(), "http://proxy:8080".into()),
                ("MY_API_KEY".into(), "value".into()),
            ]),
            &HashSet::new(),
        );
        assert_eq!(credentials, vec!["MY_API_KEY".to_string()]);
        assert_eq!(kept.len(), 1);
        assert_eq!(
            kept.get("HTTP_PROXY").map(String::as_str),
            Some("http://proxy:8080")
        );
    }
}

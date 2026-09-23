//! Known-bad profile configurations: plugin sets that DSH accepts at boot but
//! that break the instance *later*, far from their cause.
//!
//! The table below is empirical, never theoretical. Each row records a
//! configuration observed to break a real instance, what the user sees while
//! it is on, and the ids that must be off to clear it — so the check PHL
//! raises can name the symptom instead of guessing at it, and the repair it
//! offers is the same edit that was verified to fix it.
//!
//! Why this belongs to PHL at all: adoption copies a user's existing DSH_HOME
//! (and its profile) into an instance, and the plugin manager writes that same
//! `cordis.patch.yml`. A configuration that breaks DSH therefore arrives
//! *inside* PHL without anyone choosing it here, and its failure surfaces in
//! DSH's own web UI — where PHL has no view and the instance's launch log says
//! nothing. Detecting it from the patch file is the one place PHL can see it.

use std::path::{Path, PathBuf};

use super::cordis;

/// One configuration that is known to break an instance while enabled.
struct KnownConflict {
    /// One entry per plugin row, listing the spellings the *same* row can be
    /// declared with: PHL's short `- id:` handle first, then DSH's package
    /// name (DSH's own `dsh plugin add` writes the package name alone, and the
    /// loader matches on either, so a patch may carry only one of them).
    /// Order matters — the first spelling present in the file is the handle
    /// the repair edits, so it lands on the durable `- id:` row PHL owns.
    plugins: &'static [&'static [&'static str]],
    /// The plugin name as the user would recognize it.
    title: &'static str,
    /// What the user experiences while it is enabled. Stated as the symptom a
    /// bug report would carry, because that is what the user is searching for.
    symptom: &'static str,
}

/// The two rows of the Browser Use provider pair. Both must be off: the
/// registry entry alone mounts nothing, and the provider entry is what DSH
/// actually starts per session.
const BROWSER_USE_ROWS: &[&[&str]] = &[
    &["browser-use", "@deepseek-ai/dsh-browser-use"],
    &[
        "browser-use-chrome-devtools-mcp",
        "@deepseek-ai/dsh-experimental-browser-use-chrome-devtools-mcp",
    ],
];

const KNOWN: &[KnownConflict] = &[KnownConflict {
    plugins: BROWSER_USE_ROWS,
    title: "Browser Use（Chrome DevTools MCP）",
    symptom: "该实例只有第一个会话能建成：之后每次在 DSH 里新建会话都会失败（DSH 报 mcp-client(chrome-devtools-mcp) 启动失败），界面上点「新建会话」没有任何反应，磁盘上却会不断留下空白会话目录",
}];

/// One configuration found enabled in a profile's patch file.
#[derive(Debug, Clone)]
pub(crate) struct ProfileConflict {
    /// The declared spelling to turn off per enabled row — exactly one handle
    /// per row, so the repair edits each `disabled:` once.
    pub ids: Vec<String>,
    /// Title + symptom, ready to render as one check message.
    pub message: String,
}

/// The profile's `cordis.patch.yml` — the file DSH itself consults, so it is
/// the only answer that cannot drift from what actually loads.
fn patch_path(profile_dir: &Path) -> PathBuf {
    cordis::patch_path(profile_dir)
}

/// Known conflicts currently enabled in `lines` of a patch file. Takes lines
/// rather than a path so the blocking and async readers share one scan.
fn enabled_conflicts_in(lines: &[String]) -> Vec<ProfileConflict> {
    let declared = cordis::declared_plugin_ids_in(lines);
    KNOWN
        .iter()
        .filter_map(|known| {
            let ids: Vec<String> = known
                .plugins
                .iter()
                .filter_map(|spellings| {
                    spellings
                        .iter()
                        .find(|s| declared.get(**s).is_some_and(|disabled| !disabled))
                        .map(|s| (*s).to_string())
                })
                .collect();
            (!ids.is_empty()).then(|| ProfileConflict {
                ids,
                message: format!("{} 已启用：{}", known.title, known.symptom),
            })
        })
        .collect()
}

/// The enabled known conflicts in one profile directory. A missing or
/// unreadable patch is "nothing to report", not an error: a profile that has
/// never had a plugin installed legitimately has no file.
pub(crate) async fn enabled_conflicts(profile_dir: &Path) -> Vec<ProfileConflict> {
    let Ok(text) = tokio::fs::read_to_string(patch_path(profile_dir)).await else {
        return Vec::new();
    };
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    enabled_conflicts_in(&lines)
}

/// The same scan for a caller already inside a blocking file walk (the
/// adoption measure, which reads the source home synchronously).
pub(crate) fn enabled_conflicts_blocking(profile_dir: &Path) -> Vec<ProfileConflict> {
    let Ok(text) = std::fs::read_to_string(patch_path(profile_dir)) else {
        return Vec::new();
    };
    enabled_conflicts_in(&text.lines().map(str::to_owned).collect::<Vec<_>>())
}

/// Turn every enabled known conflict off, re-detecting from the file rather
/// than trusting a caller's list: a repair runs against live state, and the
/// patch may have changed since the check that offered the button.
///
/// Returns the ids disabled, for the repair outcome the UI reports.
pub(crate) async fn disable_all(profile_dir: &Path) -> Result<Vec<String>, String> {
    let mut disabled = Vec::new();
    for conflict in enabled_conflicts(profile_dir).await {
        for id in &conflict.ids {
            cordis::set_plugin_disabled(profile_dir, id, true).await?;
            disabled.push(id.clone());
        }
    }
    Ok(disabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_owned).collect()
    }

    /// The shape PHL writes: one `insert` block per plugin, `id` + `name`.
    const PHL_ENABLED: &str = r#"# profile patch
- insert:
    - id: browser-use
      name: '@deepseek-ai/dsh-browser-use'
    - id: browser-use-chrome-devtools-mcp
      name: '@deepseek-ai/dsh-experimental-browser-use-chrome-devtools-mcp'
"#;

    const PHL_DISABLED: &str = r#"- insert:
    - id: browser-use
      name: '@deepseek-ai/dsh-browser-use'
      disabled: true
    - id: browser-use-chrome-devtools-mcp
      name: '@deepseek-ai/dsh-experimental-browser-use-chrome-devtools-mcp'
      disabled: true
"#;

    #[test]
    fn detects_an_enabled_configuration_and_keeps_the_short_handle_first() {
        let found = enabled_conflicts_in(&lines(PHL_ENABLED));
        assert_eq!(found.len(), 1, "one configuration, not one per id");
        assert_eq!(
            found[0].ids,
            vec![
                "browser-use".to_string(),
                "browser-use-chrome-devtools-mcp".to_string()
            ],
            "the durable - id: handles come first; package names follow"
        );
        assert!(found[0].message.contains("Browser Use"));
    }

    /// Disabling is what clears the configuration — the check must go quiet
    /// then, or the health card would nag after its own repair.
    #[test]
    fn a_fully_disabled_configuration_is_not_a_conflict() {
        assert!(enabled_conflicts_in(&lines(PHL_DISABLED)).is_empty());
    }

    /// The shape the profile in the field actually carried: `disabled: false`
    /// written out, not the key omitted. A scan that looked for a *present*
    /// `disabled:` or compared the raw scalar text would call this one
    /// disabled and stay silent on a live break.
    #[test]
    fn an_explicit_disabled_false_is_enabled() {
        let spelled = r#"- insert:
    - id: browser-use
      name: '@deepseek-ai/dsh-browser-use'
      disabled: false
    - id: browser-use-chrome-devtools-mcp
      name: '@deepseek-ai/dsh-experimental-browser-use-chrome-devtools-mcp'
      disabled: false
"#;
        let found = enabled_conflicts_in(&lines(spelled));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].ids.len(), 2);
    }

    /// Half-disabled is still enabled: DSH loads per entry, so one live row is
    /// enough to break session creation. Only the live one is offered for edit.
    #[test]
    fn a_partially_disabled_configuration_still_reports_the_live_row() {
        let half = r#"- insert:
    - id: browser-use
      name: '@deepseek-ai/dsh-browser-use'
      disabled: true
    - id: browser-use-chrome-devtools-mcp
      name: '@deepseek-ai/dsh-experimental-browser-use-chrome-devtools-mcp'
"#;
        let found = enabled_conflicts_in(&lines(half));
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].ids,
            vec!["browser-use-chrome-devtools-mcp".to_string()]
        );
    }

    /// A patch DSH's own `dsh plugin add` wrote has no PHL handle, only the
    /// package name — detection must still catch it, and disabling must land
    /// on the id that is actually there.
    #[test]
    fn a_package_named_row_is_detected_and_editable() {
        let by_name = r#"- id: '@deepseek-ai/dsh-browser-use'
  name: '@deepseek-ai/dsh-browser-use'
"#;
        let found = enabled_conflicts_in(&lines(by_name));
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].ids,
            vec!["@deepseek-ai/dsh-browser-use".to_string()]
        );
    }

    #[test]
    fn unrelated_plugins_are_left_alone() {
        let other = "- insert:\n    - id: dsh-cool\n      name: 'dsh-cool'\n";
        assert!(enabled_conflicts_in(&lines(other)).is_empty());
        assert!(enabled_conflicts_in(&[]).is_empty());
    }

    /// The shape an adopted profile actually carried, structural details
    /// intact: two sibling rows under one `insert`, `disabled` between `id`
    /// and `name`, blank lines and comments between rows, and a nested
    /// `config:` whose quoted value contains a colon. Detection has to be
    /// right on this, and the edit has to leave the nested block alone.
    #[tokio::test]
    async fn the_field_shape_is_detected_and_edited_in_place() {
        let field = r#"- insert:
    # ── Browser Use ──────────────
    - id: browser-use
      disabled: false
      name: '@deepseek-ai/dsh-browser-use'

    - id: browser-use-chrome-devtools-mcp
      disabled: false
      name: '@deepseek-ai/dsh-experimental-browser-use-chrome-devtools-mcp'
      config:
        mode: launch
        headless: true
        executablePath: 'C:\Program Files\Google\Chrome\Application\chrome.exe'
      # 工具名形如 mcp__chrome-devtools-mcp__<tool>。

    - id: computer-use
      disabled: false
      name: '@deepseek-ai/dsh-computer-use'
"#;
        let found = enabled_conflicts_in(&lines(field));
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].ids,
            vec![
                "browser-use".to_string(),
                "browser-use-chrome-devtools-mcp".to_string()
            ],
            "the unrelated computer-use row must not be swept in"
        );

        let dir = std::env::temp_dir().join(format!(
            "phl-conflicts-field-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("cordis.patch.yml"), field)
            .await
            .unwrap();
        disable_all(&dir).await.unwrap();

        let patched = tokio::fs::read_to_string(dir.join("cordis.patch.yml"))
            .await
            .unwrap();
        assert_eq!(patched.matches("disabled: true").count(), 2);
        // `computer-use` was never a conflict, so its flag is untouched.
        assert!(patched.contains("- id: computer-use\n      disabled: false"));
        // The nested block survives byte for byte — including the colon in a
        // quoted scalar, which is what a naive key/value scan trips over.
        assert!(patched.contains(
            "      config:\n        mode: launch\n        headless: true\n        executablePath: 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'\n"
        ));
        assert!(enabled_conflicts(&dir).await.is_empty());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    /// `disable_all` must be idempotent: the health card re-runs the repair
    /// whenever a check is still repairable, and a second pass has to settle.
    #[tokio::test]
    async fn disable_all_clears_the_conflict_and_settles() {
        let dir = std::env::temp_dir().join(format!(
            "phl-conflicts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("cordis.patch.yml"), PHL_ENABLED)
            .await
            .unwrap();

        let disabled = disable_all(&dir).await.unwrap();
        assert_eq!(disabled.len(), 2);
        assert!(enabled_conflicts(&dir).await.is_empty());

        // The file must stay DSH-loadable: still a top-level array.
        let patched = tokio::fs::read_to_string(dir.join("cordis.patch.yml"))
            .await
            .unwrap();
        assert!(patched.lines().any(|l| l.trim_start().starts_with("- ")));

        assert!(
            disable_all(&dir).await.unwrap().is_empty(),
            "second pass is a no-op"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn a_profile_without_a_patch_reports_nothing() {
        let dir =
            std::env::temp_dir().join(format!("phl-conflicts-missing-{}", std::process::id()));
        assert!(enabled_conflicts(&dir).await.is_empty());
    }
}

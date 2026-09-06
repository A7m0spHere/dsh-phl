//! Line-level editing of `cordis.patch.yml`. Keeping the file byte-stable
//! outside the one plugin block — comments and unrelated entries (including
//! DSH's own) survive untouched, which a serde_yaml round-trip would not
//! guarantee.

use std::path::{Path, PathBuf};

use tauri::State;

use super::sanitize_pkg_path;
use crate::paths::PhlState;

/* -------------------------- cordis.patch.yml -------------------------- */
//
// Line-level editing keeps the file byte-stable outside the one plugin block
// — comments and unrelated entries (including DSH's own) survive untouched,
// which a serde_yaml round-trip would not guarantee.

pub(crate) fn patch_path(instance_root: &Path) -> PathBuf {
    instance_root.join("cordis.patch.yml")
}

async fn read_patch_lines(instance_root: &Path) -> Vec<String> {
    match tokio::fs::read_to_string(patch_path(instance_root)).await {
        Ok(text) => text.lines().map(|l| l.to_string()).collect(),
        Err(_) => Vec::new(),
    }
}

/// DSH scaffolds a fresh profile's patch file as a comment header plus a
/// bare `[]`. That `[]` is already a complete document: appending a
/// `- id:` block after it leaves two documents in one stream, and DSH's
/// single-document loader then refuses the file at launch ("end of the
/// stream or a document separator is expected"). A column-0 `[]` is never
/// legitimate next to entries, so every write drops it — which both lets
/// the first entry replace the placeholder and heals files a pre-fix PHL
/// had already corrupted. An *indented* `[]` (a nested empty list) is left
/// alone.
fn drop_empty_list_placeholder(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().filter(|l| l.trim_end() != "[]").collect()
}

async fn write_patch_lines(instance_root: &Path, lines: &[String]) -> Result<(), String> {
    let mut text = drop_empty_list_placeholder(lines.to_vec()).join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    tokio::fs::create_dir_all(instance_root)
        .await
        .map_err(|e| format!("实例目录不存在: {e}"))?;
    tokio::fs::write(patch_path(instance_root), text)
        .await
        .map_err(|e| e.to_string())
}

/// Registry ids currently flagged `disabled: true` in the profile's patch
/// file. The Instance Manager reads the enabled state from here rather than
/// from any record of its own — the file DSH actually consults is the only
/// answer that cannot drift.
pub(crate) async fn disabled_plugin_ids(instance_root: &Path) -> std::collections::HashSet<String> {
    let lines = read_patch_lines(instance_root).await;
    let mut disabled = std::collections::HashSet::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix("- id:") else {
            continue;
        };
        let id = rest
            .trim()
            .trim_matches(|c| c == '\'' || c == '"')
            .to_string();
        if id.is_empty() {
            continue;
        }
        let Some(range) = find_block(&lines, &id) else {
            continue;
        };
        if range.0 != i {
            continue; // a later duplicate block; the first one wins
        }
        let indent = child_indent(&lines, range);
        let flagged = (range.0 + 1..range.1).any(|j| {
            indent_of(&lines[j]) == indent
                && lines[j]
                    .trim_start()
                    .strip_prefix("disabled:")
                    .is_some_and(|v| v.trim() == "true")
        });
        if flagged {
            disabled.insert(id);
        }
    }
    disabled
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Line range `[start, end)` of the `- id: <id>` block in a top-level list.
///
/// The block ends at the next non-empty line indented at or above the item's
/// own level. Matching a trimmed `"- "` prefix instead would also stop at
/// *nested* sequence items (`      - a` under `config:`), cutting the block
/// in half and orphaning its tail at the document's top level.
fn find_block(lines: &[String], id: &str) -> Option<(usize, usize)> {
    let mut start: Option<usize> = None;
    let mut base = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        match start {
            None => {
                if let Some(rest) = trimmed.strip_prefix("- id:") {
                    if rest.trim().trim_matches(|c| c == '\'' || c == '"') == id {
                        start = Some(i);
                        base = indent_of(line);
                    }
                }
            }
            Some(s) => {
                if !trimmed.is_empty() && indent_of(line) <= base {
                    return Some((s, i));
                }
            }
        }
    }
    start.map(|s| (s, lines.len()))
}

/// Indentation of the block's direct children, so keys are read and written
/// at the right level instead of matching something nested deeper.
fn child_indent(lines: &[String], range: (usize, usize)) -> usize {
    (range.0 + 1..range.1)
        .find(|&i| !lines[i].trim().is_empty())
        .map(|i| indent_of(&lines[i]))
        .unwrap_or_else(|| indent_of(&lines[range.0]) + 2)
}

/// Writes `disabled: true/false` for the plugin block, inserting the block
/// when the plugin was never registered (e.g. an out-of-band install).
pub(crate) async fn set_plugin_disabled(
    instance_root: &Path,
    registry_id: &str,
    disabled: bool,
) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    match find_block(&lines, registry_id) {
        Some(range) => {
            // Scope the key to the block's own level: a `disabled:` sitting
            // inside a nested config mapping belongs to that sub-mapping,
            // not to the plugin.
            let indent = child_indent(&lines, range);
            let at = (range.0 + 1..range.1).find(|&i| {
                indent_of(&lines[i]) == indent && lines[i].trim_start().starts_with("disabled:")
            });
            match (disabled, at) {
                (false, Some(i)) => {
                    lines.remove(i);
                }
                (true, Some(i)) => {
                    lines[i] = format!("{}disabled: true", " ".repeat(indent));
                }
                (true, None) => {
                    lines.insert(range.0 + 1, format!("{}disabled: true", " ".repeat(indent)));
                }
                (false, None) => {}
            }
            write_patch_lines(instance_root, &lines).await
        }
        None if disabled => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("- id: {registry_id}"));
            lines.push(format!("  name: {registry_id}"));
            lines.push("  disabled: true".into());
            write_patch_lines(instance_root, &lines).await
        }
        None => Ok(()), // enabling something unregistered is a no-op
    }
}

async fn remove_plugin_block(instance_root: &Path, registry_id: &str) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    if let Some((start, end)) = find_block(&lines, registry_id) {
        lines.drain(start..end);
        write_patch_lines(instance_root, &lines).await
    } else {
        Ok(())
    }
}

/// Adds the `- id: …` entry if missing. `disabled` seeds the block with the
/// flag when the caller already knows the plugin starts disabled.
pub(crate) async fn register_cordis_patch(
    instance_root: &Path,
    registry_id: &str,
    disabled: Option<bool>,
) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    if find_block(&lines, registry_id).is_some() {
        return Ok(());
    }
    if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.push(format!("- id: {registry_id}"));
    lines.push(format!("  name: {registry_id}"));
    if disabled == Some(true) {
        lines.push("  disabled: true".into());
    }
    write_patch_lines(instance_root, &lines).await
}
#[tauri::command]
pub async fn set_plugin_enabled(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    instance_id: String,
    registry_id: String,
    enabled: bool,
) -> Result<(), String> {
    let registry_id = sanitize_pkg_path(&registry_id)?;
    let profile =
        crate::instances::writable_profile_dir(&phl.root(), &instance_id, "修改插件启用状态进")
            .await?;
    let verb = if enabled { "启用" } else { "停用" };
    crate::resources::guarded(
        crate::resources::next_task_id("plugin-toggle"),
        "plugin-toggle",
        format!("{verb}插件 {registry_id}"),
        vec![crate::resources::Resource::Instance(instance_id)],
        None,
        &locks,
        &tasks,
        move |_| async move { set_plugin_disabled(&profile, &registry_id, !enabled).await },
    )
    .await
}

#[tauri::command]
pub async fn uninstall_plugin(
    locks: State<'_, crate::resources::ResourceLocks>,
    tasks: State<'_, crate::resources::Tasks>,
    phl: State<'_, PhlState>,
    instance_id: String,
    registry_id: String,
) -> Result<(), String> {
    let registry_id = sanitize_pkg_path(&registry_id)?;
    let profile =
        crate::instances::writable_profile_dir(&phl.root(), &instance_id, "卸载插件进").await?;
    crate::resources::guarded(
        crate::resources::next_task_id("plugin-uninstall"),
        "plugin-uninstall",
        format!("卸载插件 {registry_id}"),
        vec![crate::resources::Resource::Instance(instance_id)],
        None,
        &locks,
        &tasks,
        move |_| async move {
            remove_plugin_block(&profile, &registry_id).await?;
            let dir = profile.join("node_modules").join(&registry_id);
            crate::paths::ensure_under_root(&profile.join("node_modules"), &dir)?;
            if dir.exists() {
                tokio::fs::remove_dir_all(&dir)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(())
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(|l| l.to_string()).collect()
    }

    const NESTED: &str = "\
- id: foo
  name: foo
  config:
    items:
      - a
      - b
- id: bar
  name: bar
";

    #[test]
    fn block_ends_at_a_sibling_not_at_a_nested_item() {
        let doc = lines(NESTED);
        // `      - a` is a nested sequence item; ending the block there would
        // orphan the rest of foo's config at the document's top level.
        assert_eq!(find_block(&doc, "foo"), Some((0, 6)));
        assert_eq!(find_block(&doc, "bar"), Some((6, 8)));
        assert_eq!(find_block(&doc, "missing"), None);
    }

    #[test]
    fn removing_a_block_leaves_the_rest_intact() {
        let mut doc = lines(NESTED);
        let (start, end) = find_block(&doc, "foo").unwrap();
        doc.drain(start..end);
        assert_eq!(doc, lines("- id: bar\n  name: bar\n"));
    }

    #[test]
    fn disabled_key_is_scoped_to_the_block_level() {
        let doc = lines("- id: foo\n  config:\n    disabled: true\n  name: foo\n");
        let range = find_block(&doc, "foo").unwrap();
        let indent = child_indent(&doc, range);
        assert_eq!(indent, 2);
        // The `disabled: true` at indent 4 belongs to `config`, not to foo,
        // so the flag lookup must not find it.
        let found = (range.0 + 1..range.1).find(|&i| {
            indent_of(&doc[i]) == indent && doc[i].trim_start().starts_with("disabled:")
        });
        assert_eq!(found, None);
    }

    #[test]
    fn nested_list_style_documents_are_handled() {
        // DSH may write the list under a top-level key, indenting every item.
        let doc = lines("plugins:\n  - id: foo\n    name: foo\n  - id: bar\n");
        assert_eq!(find_block(&doc, "foo"), Some((1, 3)));
        assert_eq!(child_indent(&doc, (1, 3)), 4);
    }

    fn temp_profile(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-cordis-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// serde_yaml refuses multi-document streams, same as DSH's loader — so
    /// "parses as one sequence" is the exact launch-time acceptance test.
    fn assert_single_sequence_document(text: &str) -> Vec<serde_yaml::Value> {
        serde_yaml::from_str(text).expect("patch file must be a single YAML document")
    }

    #[tokio::test]
    async fn first_entry_replaces_the_scaffold_placeholder() {
        // DSH scaffolds a fresh profile with a comment header plus `[]`.
        // Appending after that `[]` is a second document in the same stream —
        // the exact shape that made DSH exit before ready (code 1).
        let dir = temp_profile("scaffold");
        std::fs::write(
            patch_path(&dir),
            "# Your patch layer for this dsh profile\n# a top-level YAML array\n[]\n",
        )
        .unwrap();
        register_cordis_patch(&dir, "dshmarket", None)
            .await
            .unwrap();

        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        assert!(!text.contains("[]"), "placeholder replaced: {text}");
        assert!(text.contains("# Your patch layer"), "header kept: {text}");
        let doc = assert_single_sequence_document(&text);
        assert_eq!(doc[0]["id"], "dshmarket");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn writing_heals_a_file_the_old_append_corrupted() {
        // Instances hit by the pre-fix bug carry `[]` *and* entries; any
        // later plugin toggle rewrites the file, so the write path must
        // heal it, not just avoid creating new damage.
        let dir = temp_profile("heal");
        std::fs::write(
            patch_path(&dir),
            "# header\n[]\n\n- id: dshmarket\n  name: dshmarket\n",
        )
        .unwrap();
        set_plugin_disabled(&dir, "dshmarket", true).await.unwrap();

        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        assert!(!text.contains("[]"), "healed: {text}");
        let doc = assert_single_sequence_document(&text);
        assert_eq!(doc.len(), 1);
        assert_eq!(doc[0]["disabled"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn indented_empty_lists_survive_a_write() {
        // Only a column-0 `[]` is the scaffold placeholder; an indented or
        // inline empty list is real content.
        let dir = temp_profile("nested-empty");
        std::fs::write(
            patch_path(&dir),
            "- id: foo\n  name: foo\n  config:\n    items: []\n",
        )
        .unwrap();
        set_plugin_disabled(&dir, "foo", true).await.unwrap();

        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        assert!(text.contains("items: []"), "nested [] kept: {text}");
        assert_single_sequence_document(&text);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

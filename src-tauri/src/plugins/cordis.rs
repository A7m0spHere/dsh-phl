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
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim_start();
        let is_insert = trimmed.starts_with("- insert:");
        let is_id = trimmed.starts_with("- id:");
        if !is_insert && !is_id {
            i += 1;
            continue;
        }
        let item_end = find_item_end(&lines, i);
        let row_start = if is_insert {
            (i + 1..item_end).find(|&j| lines[j].trim_start().starts_with("- id:"))
        } else {
            Some(i)
        };
        let Some(row_start) = row_start else {
            i = item_end;
            continue;
        };
        let id = lines[row_start]
            .trim_start()
            .strip_prefix("- id:")
            .map(|r| r.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
            .unwrap_or_default();
        let key_indent = indent_of(&lines[row_start]) + 2;
        let flagged = (row_start + 1..item_end).any(|j| {
            indent_of(&lines[j]) == key_indent
                && lines[j]
                    .trim_start()
                    .strip_prefix("disabled:")
                    .is_some_and(|v| v.trim() == "true")
        });
        if !id.is_empty() && flagged {
            disabled.insert(id);
        }
        i = item_end;
    }
    disabled
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Line range `[start, end)` of the item that targets `id` in a top-level list.
///
/// A plugin is mounted by DSH through an `insert:` list, so the block may start
/// at `- insert:` or (legacy) at a bare `- id:` line. The block ends at the next
/// non-empty line indented at or above the item's own level. Matching a trimmed
/// `"- "` prefix instead would also stop at *nested* sequence items (`      - a`
/// under `config:`), cutting the block in half and orphaning its tail at the
/// document's top level.
fn find_block(lines: &[String], id: &str) -> Option<(usize, usize)> {
    let mut start: Option<usize> = None;
    let mut base = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        match start {
            None => {
                if trimmed.starts_with("- insert:") {
                    if insert_item_contains_id(lines, i, id) {
                        start = Some(i);
                        base = indent_of(line);
                    }
                } else if let Some(rest) = trimmed.strip_prefix("- id:") {
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

fn insert_item_contains_id(lines: &[String], start: usize, id: &str) -> bool {
    let item_end = find_item_end(lines, start);
    lines[start + 1..item_end].iter().any(|l| {
        l.trim_start()
            .strip_prefix("- id:")
            .is_some_and(|rest| rest.trim().trim_matches(|c| c == '\'' || c == '"') == id)
    })
}

fn find_item_end(lines: &[String], item_start: usize) -> usize {
    let base = indent_of(&lines[item_start]);
    (item_start + 1..lines.len())
        .find(|&i| !lines[i].trim().is_empty() && indent_of(&lines[i]) <= base)
        .unwrap_or(lines.len())
}

fn item_row_range(lines: &[String], range: (usize, usize)) -> (usize, usize) {
    // find_block returns the top-level item range, which for an insert block
    // begins at `- insert:`. Row-level work (child indent, the `disabled:`
    // key) happens on the nested `- id:` row, so skip the insert opener.
    if lines[range.0].trim_start().starts_with("- insert:") {
        if let Some(i) =
            (range.0 + 1..range.1).find(|&i| lines[i].trim_start().starts_with("- id:"))
        {
            return (i, range.1);
        }
    }
    range
}

/// Indentation of the block's direct children, so keys are read and written
/// at the right level instead of matching something nested deeper.
fn child_indent(lines: &[String], range: (usize, usize)) -> usize {
    let row = item_row_range(lines, range);
    (row.0 + 1..row.1)
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
            let row = item_row_range(&lines, range);
            // Scope the key to the plugin row's own level: a `disabled:`
            // sitting inside a nested `config:` mapping belongs to that
            // sub-mapping, not to the plugin.
            let indent = child_indent(&lines, range);
            let at = (row.0 + 1..range.1).find(|&i| {
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
                    lines.insert(row.0 + 1, format!("{}disabled: true", " ".repeat(indent)));
                }
                (false, None) => {}
            }
            write_patch_lines(instance_root, &lines).await
        }
        None if disabled => {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.extend(insert_block(registry_id, true));
            write_patch_lines(instance_root, &lines).await
        }
        None => Ok(()), // enabling something unregistered is a no-op
    }
}

/// The patch lines that mount one plugin through DSH's `insert` channel. PHL
/// uses the package name as both the loader `id` and `name`: the plugin is
/// resolved by `name`, and `id` is PHL's stable handle for enable/disable/
/// uninstall. Writing a bare `- id:` (the old shape) instead makes DSH read it
/// as "override a plugin that already exists" and silently drop it, so the
/// plugin installs but never mounts.
fn insert_block(registry_id: &str, disabled: bool) -> Vec<String> {
    let mut block = vec![
        "- insert:".to_string(),
        format!("    - id: {registry_id}"),
        format!("      name: '{registry_id}'"),
    ];
    if disabled {
        block.push("      disabled: true".to_string());
    }
    block
}

async fn remove_plugin_block(instance_root: &Path, registry_id: &str) -> Result<(), String> {
    let mut lines = read_patch_lines(instance_root).await;
    if let Some((start, end)) = find_block(&lines, registry_id) {
        lines.drain(start..end);
        trim_trailing_blanks(&mut lines);
        write_patch_lines(instance_root, &lines).await
    } else {
        Ok(())
    }
}

fn trim_trailing_blanks(lines: &mut Vec<String>) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
}

/// Mounts the plugin through DSH's `insert` channel if it is not already there.
/// `disabled` seeds the block with the flag when the caller already knows the
/// plugin starts disabled.
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
    lines.extend(insert_block(registry_id, disabled == Some(true)));
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
    let profile = crate::instances::profile_dir(&phl.root(), &instance_id).await?;
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
    let profile = crate::instances::profile_dir(&phl.root(), &instance_id).await?;
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
        assert_eq!(doc[0]["insert"][0]["id"], "dshmarket");
        assert_eq!(doc[0]["insert"][0]["name"], "dshmarket");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn register_mounts_through_the_insert_channel() {
        // The whole point of the fix: an installed plugin must be added with an
        // `insert` list. A bare `- id:` entry is read by DSH as "override a
        // plugin that already exists" and dropped, so it installs but never
        // mounts. Assert the emitted file actually carries an insert.
        let dir = temp_profile("mount");
        register_cordis_patch(&dir, "dsh-better-sidebar", None)
            .await
            .unwrap();
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        let doc = assert_single_sequence_document(&text);
        assert!(
            doc[0].get("insert").is_some(),
            "must be an insert block: {text}"
        );
        assert_eq!(doc[0]["insert"][0]["name"], "dsh-better-sidebar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn enable_disable_roundtrip_on_an_insert_block() {
        let dir = temp_profile("toggle");
        register_cordis_patch(&dir, "dsh-dream-skin", None)
            .await
            .unwrap();

        // The plugin is mounted and enabled at first.
        assert!(disabled_plugin_ids(&dir).await.is_empty());

        set_plugin_disabled(&dir, "dsh-dream-skin", true)
            .await
            .unwrap();
        let disabled = disabled_plugin_ids(&dir).await;
        assert!(
            disabled.contains("dsh-dream-skin"),
            "flag not read back: {disabled:?}"
        );
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        assert_single_sequence_document(&text);

        set_plugin_disabled(&dir, "dsh-dream-skin", false)
            .await
            .unwrap();
        assert!(disabled_plugin_ids(&dir).await.is_empty());
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        assert!(!text.contains("disabled: true"), "flag cleared: {text}");
        assert!(
            text.contains("    - id: dsh-dream-skin"),
            "still mounted: {text}"
        );
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

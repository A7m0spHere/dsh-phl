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
    // Write beside the target and rename, like the instance manifest and the
    // API config: DSH loads this file at launch, so a crash mid-write must not
    // leave a truncated patch it cannot parse. Plugin writes are serialized by
    // the instance resource lock, so one temp name per instance is safe.
    let path = patch_path(instance_root);
    let tmp = path.with_extension("yml.tmp");
    tokio::fs::write(&tmp, text)
        .await
        .map_err(|e| format!("无法写入插件配置: {e}"))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| format!("无法写入插件配置: {e}"))
}

/// Registry ids currently flagged `disabled: true` in the profile's patch
/// file. The Instance Manager reads the enabled state from here rather than
/// from any record of its own — the file DSH actually consults is the only
/// answer that cannot drift.
pub(crate) async fn disabled_plugin_ids(instance_root: &Path) -> std::collections::HashSet<String> {
    declared_plugin_ids(instance_root)
        .await
        .into_iter()
        .filter_map(|(id, disabled)| disabled.then_some(id))
        .collect()
}

/// Only the fields installation changes are snapshotted. Restoring this
/// state must not replace unrelated plugins or user configuration in a patch.
pub(crate) async fn plugin_registration_state(
    profile: &Path,
    registry_id: &str,
) -> Result<Option<bool>, String> {
    let text = match tokio::fs::read_to_string(patch_path(profile)).await {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("无法备份插件注册状态: {e}")),
    };
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    Ok(mount_rows(&lines).into_iter().find_map(|row| {
        let matches = ["id", "name"].iter().any(|key| {
            row_value(&lines, row.range, key)
                == Some(serde_yaml::Value::String(registry_id.to_owned()))
        });
        matches.then(|| {
            row_value(&lines, row.range, "disabled") == Some(serde_yaml::Value::Bool(true))
        })
    }))
}

pub(crate) async fn restore_plugin_registration_state(
    profile: &Path,
    registry_id: &str,
    previous: Option<bool>,
) -> Result<(), String> {
    // Do not turn a transient read error into an empty patch on recovery.
    let _ = plugin_registration_state(profile, registry_id).await?;
    match previous {
        Some(disabled) => {
            register_cordis_patch(profile, registry_id, Some(disabled)).await?;
            set_plugin_disabled(profile, registry_id, disabled).await
        }
        None => remove_plugin_block(profile, registry_id).await,
    }
}

pub(crate) async fn declared_plugin_ids(profile: &Path) -> std::collections::HashMap<String, bool> {
    let lines = read_patch_lines(profile).await;
    let mut out = std::collections::HashMap::new();
    for row in mount_rows(&lines) {
        let range = row.range;
        let disabled = row_value(&lines, range, "disabled")
            .is_some_and(|v| v == serde_yaml::Value::Bool(true));
        for key in ["id", "name"] {
            if let Some(serde_yaml::Value::String(id)) = row_value(&lines, range, key) {
                out.entry(id).or_insert(disabled);
            }
        }
    }
    out
}

pub(crate) fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

struct MountRow {
    range: (usize, usize),
    // Removing a sole row also removes its now-empty insert wrapper.
    block: (usize, usize),
}

/// Only direct list children are mount rows; lists inside config are opaque.
fn mount_rows(lines: &[String]) -> Vec<MountRow> {
    let Some(base) = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("- "))
        .map(|l| indent_of(l))
        .min()
    else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if indent_of(&lines[i]) != base || !lines[i].trim_start().starts_with("- ") {
            i += 1;
            continue;
        }
        let end = find_item_end(lines, i);
        if lines[i].trim_start().starts_with("- insert:") {
            let child_base = (i + 1..end)
                .filter(|&j| lines[j].trim_start().starts_with("- "))
                .map(|j| indent_of(&lines[j]))
                .min();
            if let Some(child_base) = child_base {
                let starts: Vec<usize> = (i + 1..end)
                    .filter(|&j| {
                        indent_of(&lines[j]) == child_base
                            && lines[j].trim_start().starts_with("- ")
                    })
                    .collect();
                for &start in &starts {
                    let range = (start, find_item_end(lines, start).min(end));
                    rows.push(MountRow {
                        range,
                        block: if starts.len() == 1 { (i, end) } else { range },
                    });
                }
            }
        } else {
            rows.push(MountRow {
                range: (i, end),
                block: (i, end),
            });
        }
        i = end;
    }
    rows
}

fn row_value(lines: &[String], range: (usize, usize), key: &str) -> Option<serde_yaml::Value> {
    let prefix = format!("{key}:");
    let first = lines[range.0].trim_start().strip_prefix("- ")?;
    let raw = first.strip_prefix(&prefix).or_else(|| {
        (range.0 + 1..range.1).find_map(|i| {
            (indent_of(&lines[i]) == indent_of(&lines[range.0]) + 2)
                .then(|| lines[i].trim_start().strip_prefix(&prefix))
                .flatten()
        })
    })?;
    serde_yaml::from_str(raw.trim()).ok()
}

/// Return just the matched row when an insert contains sibling plugins.
pub(crate) fn find_block(lines: &[String], id: &str) -> Option<(usize, usize)> {
    mount_rows(lines)
        .into_iter()
        .find(|row| {
            ["id", "name"].into_iter().any(|key| {
                row_value(lines, row.range, key)
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .as_deref()
                    == Some(id)
            })
        })
        .map(|row| row.block)
}

pub(crate) fn find_item_end(lines: &[String], item_start: usize) -> usize {
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
        if let Some(i) = (range.0 + 1..range.1).find(|&i| lines[i].trim_start().starts_with("- ")) {
            return (i, range.1);
        }
    }
    range
}

/// Indentation of the block's direct children, so keys are read and written
/// at the right level instead of matching something nested deeper.
fn child_indent(lines: &[String], range: (usize, usize)) -> usize {
    indent_of(&lines[item_row_range(lines, range).0]) + 2
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
    // YAML plain scalars cannot start with @ (scoped npm package names).
    let loader_id = if registry_id.starts_with('@') {
        format!("'{registry_id}'")
    } else {
        registry_id.to_string()
    };
    let mut block = vec![
        "- insert:".to_string(),
        format!("    - id: {loader_id}"),
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
    let profile =
        crate::instances::writable_profile_dir(&phl.root(), &instance_id, "修改插件于").await?;
    let verb = if enabled { "启用" } else { "停用" };
    crate::resources::guarded(
        crate::resources::next_task_id("plugin-toggle"),
        "plugin-toggle",
        format!("{verb}插件 {registry_id}"),
        vec![crate::resources::Resource::Instance(instance_id)],
        None,
        &locks,
        &tasks,
        move |_| async move {
            super::install::recover_profile_transaction(&profile).await?;
            set_plugin_disabled(&profile, &registry_id, !enabled).await
        },
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
        crate::instances::writable_profile_dir(&phl.root(), &instance_id, "修改插件于").await?;
    crate::resources::guarded(
        crate::resources::next_task_id("plugin-uninstall"),
        "plugin-uninstall",
        format!("卸载插件 {registry_id}"),
        vec![crate::resources::Resource::Instance(instance_id)],
        None,
        &locks,
        &tasks,
        move |task| async move {
            super::install::recover_profile_transaction(&profile).await?;
            remove_plugin_block(&profile, &registry_id).await?;
            let nm = profile.join("node_modules");
            let dir = nm.join(&registry_id);
            crate::paths::ensure_under_root(&nm, &dir)?;
            task.set_phase("removing");
            crate::instances::remove_tree_progress(&dir, &task)
                .await
                .map_err(|(_, e)| e.to_string())?;
            // Recovery above consumes verified backups before unregistering;
            // never guess ownership from flattened package names here.
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

    #[tokio::test]
    async fn shared_insert_keeps_siblings_and_ignores_nested_config_rows() {
        let dir = temp_profile("shared-insert");
        std::fs::write(patch_path(&dir), "- insert:\n    - name: '@acme/first'\n      id: first\n      config:\n        entries:\n          - id: nested\n            name: ghost\n    - id: second\n      name: second\n      disabled: true # retained sibling\n").unwrap();
        set_plugin_disabled(&dir, "@acme/first", true)
            .await
            .unwrap();
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        let doc = assert_single_sequence_document(&text);
        assert_eq!(doc[0]["insert"][0]["disabled"], true);
        assert_eq!(doc[0]["insert"][1]["disabled"], true);
        assert!(find_block(&lines(&text), "ghost").is_none());
        let disabled = disabled_plugin_ids(&dir).await;
        assert!(disabled.contains("@acme/first"));
        assert!(disabled.contains("second"));
        remove_plugin_block(&dir, "@acme/first").await.unwrap();
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        let doc = assert_single_sequence_document(&text);
        assert_eq!(doc[0]["insert"].as_sequence().unwrap().len(), 1);
        assert_eq!(doc[0]["insert"][0]["name"], "second");
        set_plugin_disabled(&dir, "second", false).await.unwrap();
        assert!(disabled_plugin_ids(&dir).await.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn id_only_insert_gets_a_row_level_disabled_key() {
        let dir = temp_profile("id-only");
        std::fs::write(patch_path(&dir), "- insert:\n    - id: foo\n").unwrap();
        set_plugin_disabled(&dir, "foo", true).await.unwrap();
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        let doc = assert_single_sequence_document(&text);
        assert_eq!(doc[0]["insert"][0]["disabled"], true);
        std::fs::remove_dir_all(dir).unwrap();
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
    async fn find_block_matches_the_mount_name_for_a_handwritten_id() {
        // Out-of-band installs write `- insert:` with a loader `id` that
        // differs from the npm package `name`. The scan reports the package
        // dir as the handle, so a disable must locate the block by `name` too
        // — else it appends a second block and DSH inserts the package twice.
        let dir = temp_profile("name-handle");
        std::fs::write(
            patch_path(&dir),
            "- insert:\n    - id: dsh-market\n      name: dshmarket\n",
        )
        .unwrap();
        set_plugin_disabled(&dir, "dshmarket", true).await.unwrap();
        let text = std::fs::read_to_string(patch_path(&dir)).unwrap();
        let doc = assert_single_sequence_document(&text);
        assert_eq!(
            doc.len(),
            1,
            "the existing block is toggled, not duplicated: {text}"
        );
        assert_eq!(doc[0]["insert"][0]["id"], "dsh-market");
        assert_eq!(doc[0]["insert"][0]["name"], "dshmarket");
        assert_eq!(doc[0]["insert"][0]["disabled"], true);
        // And the flag reads back under both the loader id and the handle.
        assert!(disabled_plugin_ids(&dir).await.contains("dsh-market"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn writing_the_patch_file_leaves_no_temp_behind() {
        // The write is tmp+rename so a crash cannot leave a truncated patch
        // that DSH refuses to parse; the temp file must not survive a success.
        let root = temp_profile("patch-atomic");
        write_patch_lines(&root, &["- id: demo".to_string()])
            .await
            .unwrap();

        let written = tokio::fs::read_to_string(patch_path(&root)).await.unwrap();
        assert!(written.contains("id: demo"));
        let leftovers: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&root);
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

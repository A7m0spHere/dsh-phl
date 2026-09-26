//! Detection and the read-only plan: open the container, cross-check the
//! root marker against the manifest, walk every entry for safety and
//! target collisions, and resolve each dependency's real package identity.

use std::io::Read;
use std::path::Path;

use super::{
    author_text, description_text, forbidden_entry, unsafe_entry, CommunityManifest,
    CommunityPackPreview, PackDependency, MAX_ENTRIES, MAX_MANIFEST_BYTES, MAX_PACK_FILE_BYTES,
};
use sha2::{Digest, Sha256};

pub(crate) type OpenedPack = (
    zip::ZipArchive<std::io::BufReader<std::fs::File>>,
    CommunityManifest,
    u32,
    String,
    u64,
);

/// Opens the pack, validates the container + manifest, and returns the
/// parsed pieces. All of it is read-only.
pub(crate) fn open_pack(path: &Path) -> Result<OpenedPack, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("无法读取包文件: {e}"))?;
    if meta.len() > MAX_PACK_FILE_BYTES {
        return Err(format!(
            "包文件超过 {} 上限",
            format_bytes(MAX_PACK_FILE_BYTES)
        ));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("无法打开包文件: {e}"))?;
    let mut reader = std::io::BufReader::new(file);
    let mut sha = Sha256::new();
    {
        use std::io::Seek;
        let mut buf = [0u8; 65536];
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|e| format!("读取包文件失败: {e}"))?;
            if n == 0 {
                break;
            }
            sha.update(&buf[..n]);
        }
        reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
    }
    let pack_sha256 = hex::encode(sha.finalize());

    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("不是有效的 ZIP 容器: {e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("包内条目数超过上限 {MAX_ENTRIES}"));
    }

    // Root marker: the container version lives here, never in the extension.
    let marker = read_root_json(&mut archive, "dspack.json")?;
    let marker_obj =
        marker.ok_or("根目录缺少 dspack.json 标记：这不是 .dspack 包（外来 ZIP 或损坏）")?;
    let format = marker_obj
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if format != "dspack" {
        return Err("dspack.json 的 format 不是 \"dspack\"：拒绝载入".into());
    }
    let container_version = marker_obj
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or("dspack.json 缺少 version 字段")?;
    if container_version != 2 && container_version != 3 {
        return Err(format!(
            "不支持该 .dspack 容器版本 {container_version}（本版支持 v2/v3）"
        ));
    }

    // Ambiguity guard: more than one root JSON carrying manifestVersion is a
    // conflict — refuse instead of picking whichever the directory order
    // happened to yield.
    let mut manifest_candidates: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if !name.ends_with(".json") || name.contains('/') {
            continue;
        }
        if name == "dspack.json" || name == "package.json" {
            continue;
        }
        let size = entry.size() as usize;
        if size > MAX_MANIFEST_BYTES {
            return Err("根目录 JSON 超过 manifest 大小上限".into());
        }
        let mut text = String::new();
        if entry
            .take(size.min(MAX_MANIFEST_BYTES) as u64)
            .read_to_string(&mut text)
            .is_ok()
        {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                if v.get("manifestVersion").is_some() {
                    manifest_candidates.push(name);
                }
            }
        }
    }
    if manifest_candidates.len() > 1 {
        return Err(format!(
            "根目录存在多个候选 manifest（{}）：判定歧义，拒绝导入",
            manifest_candidates.join("、")
        ));
    }
    let manifest_bytes =
        read_root_json(&mut archive, "manifest.json")?.ok_or("根目录缺少 manifest.json")?;
    let manifest_text = serde_json::to_string(&manifest_bytes).map_err(|e| e.to_string())?;
    let manifest: CommunityManifest =
        serde_json::from_str(&manifest_text).map_err(|e| format!("manifest.json 解析失败: {e}"))?;
    Ok((
        archive,
        manifest,
        container_version as u32,
        pack_sha256,
        meta.len(),
    ))
}

fn read_root_json(
    archive: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
    name: &str,
) -> Result<Option<serde_json::Value>, String> {
    let found = (0..archive.len()).find(|i| {
        archive
            .by_index(*i)
            .map(|e| e.name() == name)
            .unwrap_or(false)
    });
    let Some(index) = found else {
        return Ok(None);
    };
    let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
    if entry.size() > MAX_MANIFEST_BYTES as u64 {
        return Err(format!("{name} 超过大小上限"));
    }
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|_| format!("{name} 不是 UTF-8 文本"))?;
    let value =
        serde_json::from_str(&text).map_err(|e| format!("{name} 不是有效的 JSON 对象: {e}"))?;
    Ok(Some(value))
}

fn format_bytes(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", units[unit])
    }
}

/// Resolves the npm package name a git coordinate materialises under. The
/// repo's last path segment is NOT the package name by contract — the only
/// in-pack evidence is the bundle stack (the bundle DSH loads *is* the
/// package DSH must find). No other inference is honest: an ambiguous pack
/// is refused, never guessed (R2-01).
fn git_package_name(coordinate: &str, bundles: &[String]) -> Result<String, String> {
    let repo_last = coordinate
        .strip_prefix("github:")
        .and_then(|r| r.split('/').next_back())
        .unwrap_or(coordinate)
        .to_string();
    if bundles.iter().any(|b| b.as_str() == repo_last) {
        return Ok(repo_last);
    }
    Err(format!(
        "无法确定 git 依赖 {coordinate} 的 npm 包名（仓库名 {repo_last} 不在 bundle 栈中）：\
         请使用包名与仓库名一致的依赖或联系包作者修正"
    ))
}

pub(crate) fn build_preview(path: &Path) -> Result<CommunityPackPreview, String> {
    let (mut archive, manifest, container_version, pack_sha256, pack_size) = open_pack(path)?;

    let mut blocked: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    let pack_type = manifest.r#type.clone().unwrap_or_else(|| {
        if manifest.manifest_version == 4 {
            "profile".into() // v4 falls back to profile per spec
        } else {
            String::new()
        }
    });
    if pack_type == "dshhome" {
        blocked.push("type: dshhome（完整 DSH_HOME 快照）本版不支持：需要多 profile 与全局内容映射，将在后续版本支持".into());
    } else if pack_type == "collection" {
        blocked.push("type: collection 是规范保留类型，校验器直接拒绝".into());
    } else if pack_type != "profile" {
        blocked.push(format!("未知包类型 {pack_type:?}，拒绝导入"));
    }
    if manifest.manifest_version != 4 && manifest.manifest_version != 5 {
        blocked.push(format!(
            "不支持该 manifest 版本 {}（本版支持 v4/v5，profile 包）",
            manifest.manifest_version
        ));
    }

    // files[]: the wire field is an array; a non-empty one is R2-A.
    if let Some(files) = &manifest.files {
        if !files.is_empty() {
            blocked.push(format!(
                "包含 {} 条外部下载资源（files[]）：本版不支持联网下载，将在下一版支持",
                files.len()
            ));
        }
    }

    // Dependency identity first: bundle membership, git package names.
    let mut dependencies: Vec<PackDependency> = Vec::new();
    for (coordinate, pin) in &manifest.dependencies {
        let is_git = coordinate.starts_with("github:");
        let pin_state = if is_git {
            let hex_only = pin.chars().all(|c| c.is_ascii_hexdigit());
            let full = pin.len() == 40 && hex_only;
            let short = !pin.is_empty() && pin.len() < 40 && hex_only;
            if full {
                "full"
            } else if short {
                // Resolved to the unique full commit at prepare time (brief
                // §10.4); a pack with an unresolvable short SHA blocks there.
                "short-unresolved"
            } else {
                blocked.push(format!(
                    "git 依赖 {coordinate} 的 pin {pin:?} 既不是完整也不是可解析的短 commit SHA"
                ));
                "invalid"
            }
        } else {
            "full"
        };
        let package_name = if is_git {
            match git_package_name(coordinate, &manifest.bundles) {
                Ok(name) => name,
                Err(reason) => {
                    blocked.push(reason);
                    String::new()
                }
            }
        } else {
            coordinate.clone()
        };
        dependencies.push(PackDependency {
            coordinate: coordinate.clone(),
            pin: pin.clone(),
            kind: if is_git { "git" } else { "npm" }.into(),
            in_bundles: manifest.bundles.iter().any(|b| b == coordinate),
            package_name,
            pin_state: pin_state.into(),
        });
    }

    // Reconcile the bundle stack (the launcher guide's reconcileProfile):
    // every non-template bundle must have a dependency to materialise.
    for bundle in &manifest.bundles {
        let known = dependencies
            .iter()
            .any(|d| d.package_name == *bundle || &d.coordinate == bundle);
        if !known {
            // Template bundles (dsh-base, dsh-web-app…) are DSH's own; a
            // non-template name with no dependency is the loader's "missing"
            // case → refuse rather than boot a broken stack.
            let looks_template = bundle.starts_with("@deepseek-ai/");
            if !looks_template {
                blocked.push(format!(
                    "bundle {bundle} 在 bundle 栈中但没有任何依赖提供它：加载层栈不完整，拒绝导入"
                ));
            }
        }
    }

    // Entry walk: counts, security re-check, boundary checks, and the
    // cross-namespace target collision map (CR-03).
    let mut overrides_count = 0usize;
    let mut home_count = 0usize;
    let mut forbidden: Vec<String> = Vec::new();
    let mut saw_patch_file = false;
    let mut collisions: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        let namespace = if name.starts_with("overrides/") {
            overrides_count += 1;
            "overrides"
        } else if name.starts_with("home/") {
            if container_version < 3 {
                blocked.push("容器 v2 不允许 home/ 目录（v3 才引入）".into());
            }
            home_count += 1;
            "home"
        } else {
            if name.starts_with("files/") {
                blocked.push("包内含 files/ 实体：外部资源本版不支持".into());
            } else if !matches!(
                name.as_str(),
                "dspack.json"
                    | "manifest.json"
                    | "package.json"
                    | "pnpm-workspace.yaml"
                    | "pnpm-lock.yaml"
            ) {
                warnings.push(format!("包内未识别的根文件 {name} 将被忽略"));
            }
            continue;
        };
        let rel = name
            .strip_prefix("overrides/")
            .or_else(|| name.strip_prefix("home/"))
            .unwrap_or("");
        if unsafe_entry(rel) {
            blocked.push(format!("不安全的归档路径 {name}：拒绝导入"));
            continue;
        }
        if let Some(reason) = forbidden_entry(rel) {
            forbidden.push(format!("{name}（{reason}）"));
            continue;
        }
        if namespace == "overrides" && rel == "cordis.patch.yml" {
            saw_patch_file = true;
        }
        // Target-collision map keyed on the normalized HOME-relative path.
        let target = super::target_rel_path(namespace, rel)
            .ok_or_else(|| format!("无法归一化目标路径 {name}"))?;
        collisions.entry(target).or_default().push(name);
    }
    if !forbidden.is_empty() {
        blocked.push(format!(
            "包内包含禁止落盘的内容，拒绝导入：{}",
            forbidden.join("；")
        ));
    }
    let target_collisions: Vec<String> = collisions
        .iter()
        .filter(|(_, sources)| sources.len() > 1)
        .map(|(target, sources)| format!("{target} ← {}", sources.join(" 与 ")))
        .collect();
    if !target_collisions.is_empty() {
        blocked.push(format!(
            "多个归档条目映射到同一目标文件，无明确优先级契约，拒绝导入：{}",
            target_collisions.join("；")
        ));
    }

    let patch_source = if saw_patch_file {
        "overrides/cordis.patch.yml（文件优先）".into()
    } else if manifest.patch.is_some() {
        "manifest.patch 字段".into()
    } else {
        "无 patch 层".into()
    };

    let template_bundles: Vec<String> = manifest
        .bundles
        .iter()
        .filter(|b| !manifest.dependencies.contains_key(*b))
        .cloned()
        .collect();

    Ok(CommunityPackPreview {
        format: "dspack".into(),
        container_version,
        manifest_version: manifest.manifest_version,
        pack_type,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        display_name: manifest.display_name.clone(),
        author: author_text(&manifest.author),
        description: description_text(&manifest.description),
        profile_name: manifest
            .profile_name
            .clone()
            .unwrap_or_else(|| "pack".into()),
        dsh_version: manifest
            .dsh_version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        bundles: manifest.bundles.clone(),
        template_bundles,
        dependencies,
        patch_source,
        overrides_count,
        home_count,
        pack_sha256,
        pack_size,
        blocked,
        warnings,
        target_collisions,
    })
}

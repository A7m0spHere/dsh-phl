//! `phl-pack` — the command line around the `.phlpack` format (development
//! spec §15, P2-1). One format, one implementation: everything this tool does
//! goes through `phl-pack-core`, the exact crate the PHL desktop app uses to
//! validate and install packs. That is the spec's rule — "Skill 不应该自己发明
//! Pack 格式 … 不要长期维护两套 Pack 实现" — enforced by there being only one.
//!
//! The primary consumer is the `phl-export` DSH Skill (part 14): a DSH user
//! without PHL installs the Skill, the Skill inspects the environment and
//! assembles a staging directory, and this CLI seals it into a `.phlpack`.
//!
//! Subcommands (spec §30): `inspect`, `validate`, `unpack`, `build`.
//! Deliberately hand-rolled argument parsing — four fixed verbs do not earn a
//! clap dependency, and the offline MSRV-1.77 build pins its dep graph tight.

use std::path::PathBuf;
use std::process::ExitCode;

use phl_pack_core::{unpack, PluginSource, ValidatedPack, PACK_FORMAT_VERSION};

const USAGE: &str = "用法: phl-pack <命令> [参数]

命令:
  inspect  <pack> [--json]        查看整合包元数据（--json 输出完整 manifest）
  validate <pack>                 完整校验（结构/路径/大小/integrity），退出码报告结果
  unpack   <pack> <目标目录>       把全部载荷解到目标目录（embedded/plugins、sessions、overrides）
  build    <布局目录> <输出.phlpack>  从 §6.1 布局目录（含 phlpack.json）打包并自校验

命令与格式规格: docs/skills/phl-export/SKILL.md（phl-export Skill 即调用本工具）。";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("inspect") => cmd_inspect(&args[1..]),
        Some("validate") => cmd_validate(&args[1..]),
        Some("unpack") => cmd_unpack(&args[1..]),
        Some("build") => cmd_build(&args[1..]),
        Some("--help") | Some("-h") | Some("help") => {
            println!("{USAGE}");
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            Err("未知命令".to_string())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("错误: {message}");
            ExitCode::FAILURE
        }
    }
}

/// One arg: a pack path that must exist (a nicer error than the zip opener's).
fn pack_arg(args: &[String], verb: &str) -> Result<PathBuf, String> {
    let raw = args
        .first()
        .ok_or_else(|| format!("用法: phl-pack {verb} <pack>"))?;
    let path = PathBuf::from(raw);
    if !path.is_file() {
        return Err(format!("文件不存在: {}", path.display()));
    }
    Ok(path)
}

fn cmd_inspect(args: &[String]) -> Result<(), String> {
    let pack = pack_arg(args, "inspect")?;
    let validated = phl_pack_core::read_pack_from_path(&pack).map_err(|e| e.detail())?;
    let json = args.iter().any(|a| a == "--json");
    if json {
        let doc = serde_json::to_value(&validated.manifest).map_err(|e| e.to_string())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    print_summary(&validated);
    Ok(())
}

fn print_summary(validated: &ValidatedPack) {
    let m = &validated.manifest;
    println!("{} v{}（id: {}）", m.pack.name, m.pack.version, m.pack.id);
    if !m.pack.author.trim().is_empty() {
        println!("作者: {}", m.pack.author);
    }
    if !m.pack.description.trim().is_empty() {
        println!("说明: {}", m.pack.description);
    }
    println!(
        "格式: phlpack v{}（本工具支持 ≤ v{PACK_FORMAT_VERSION}）",
        m.format_version
    );
    println!("DSH: {}", m.dsh.version);
    println!(
        "Runtime: {}{}",
        m.runtime.kind.as_deref().unwrap_or("node"),
        m.runtime
            .node_version
            .as_ref()
            .map(|v| format!(" {v}"))
            .unwrap_or_default()
    );
    if let Some(arch) = &m.runtime.arch {
        println!("架构: {arch}");
    }
    let remote = m
        .plugins
        .iter()
        .filter(|p| matches!(p.source, PluginSource::Registry { .. }))
        .count();
    let embedded = m.plugins.len() - remote;
    println!(
        "插件: {}（在线 {remote} · 内置 {embedded}）",
        m.plugins.len()
    );
    for p in &m.plugins {
        let source = match &p.source {
            PluginSource::Registry { registry_id } => format!("registry:{registry_id}"),
            PluginSource::Embedded { path } => format!("embedded:{path}"),
        };
        let required = if p.required { "" } else { " · 可选" };
        println!("  - {} {} [{source}]{required}", p.id, p.version);
    }
    if m.content.sessions_included || validated.has_sessions {
        let count = m
            .content
            .session_count
            .map(|c| c.to_string())
            .unwrap_or_else(|| "若干".into());
        println!("历史对话: {count} 条 ⚠ 包含用户对话数据");
    } else {
        println!("历史对话: 不包含");
    }
    println!(
        "凭据: {}",
        if m.content.secrets_excluded {
            "导出时已剥离（安装后需重新配置）"
        } else {
            "未声明已剥离 — 安装方仍会按凭据名单过滤"
        }
    );
    if let Some(integrity) = &m.integrity {
        println!("完整性: sha256 覆盖 {} 个文件", integrity.len());
    }
    println!("条目: {} 个文件", validated.entries.len());
}

fn cmd_validate(args: &[String]) -> Result<(), String> {
    let pack = pack_arg(args, "validate")?;
    let validated = phl_pack_core::read_pack_from_path(&pack).map_err(|e| e.detail())?;
    println!(
        "有效: {} v{} · {} 个条目 · integrity {}",
        validated.manifest.pack.id,
        validated.manifest.pack.version,
        validated.entries.len(),
        if validated.manifest.integrity.is_some() {
            "已校验"
        } else {
            "（无）"
        }
    );
    Ok(())
}

fn cmd_unpack(args: &[String]) -> Result<(), String> {
    let (Some(pack), Some(dest)) = (args.first(), args.get(1)) else {
        return Err("用法: phl-pack unpack <pack> <目标目录>".into());
    };
    let pack = pack_arg(std::slice::from_ref(pack), "unpack")?;
    // Validate first — `unpack` must never extract bytes the installer would
    // refuse (the same "validate before writing" rule as PHL's own install).
    phl_pack_core::read_pack_from_path(&pack).map_err(|e| e.detail())?;
    let dest = PathBuf::from(dest);
    std::fs::create_dir_all(&dest).map_err(|e| format!("创建目标目录失败: {e}"))?;
    let written = unpack::unpack_entries_to(&pack, |rel| Some((dest.join(rel), dest.clone())))
        .map_err(|e| e.detail())?;
    println!("已解包 {} 个文件到 {}", written.len(), dest.display());
    Ok(())
}

fn cmd_build(args: &[String]) -> Result<(), String> {
    let (Some(src), Some(out)) = (args.first(), args.get(1)) else {
        return Err("用法: phl-pack build <布局目录> <输出.phlpack>".into());
    };
    let src = PathBuf::from(src);
    if !src.is_dir() {
        return Err(format!("目录不存在: {}", src.display()));
    }
    let out = PathBuf::from(out);
    if out.exists() {
        return Err(format!(
            "输出文件已存在（不静默覆盖，规格 §16 的覆盖确认归调用方）: {}",
            out.display()
        ));
    }
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建输出目录失败: {e}"))?;
    }
    let mut withheld = Vec::new();
    let mut skipped_links = Vec::new();
    let validated =
        phl_pack_core::write::build_pack_from_dir(&src, &out, &mut withheld, &mut skipped_links)
            .map_err(|e| e.detail())?;
    println!(
        "已生成 {}: {} v{} · {} 个条目（已通过完整校验）",
        out.display(),
        validated.manifest.pack.id,
        validated.manifest.pack.version,
        validated.entries.len()
    );
    if !skipped_links.is_empty() {
        skipped_links.sort();
        skipped_links.dedup();
        println!(
            "ℹ 未打包 {} 个文件系统链接（整合包只携带字节，不携带机器本地路径）：{}",
            skipped_links.len(),
            skipped_links.join("、")
        );
    }
    if !withheld.is_empty() {
        withheld.sort();
        withheld.dedup();
        println!(
            "⚠ 已按规格 §12 扣下 {} 个疑似凭据文件（未打包）：{}",
            withheld.len(),
            withheld.join("、")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A staging layout in the air: manifest + one embedded plugin + a
    /// session. Same shape the Skill produces (docs/skills/phl-export).
    fn fixture_layout(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("embedded/plugins/mine")).unwrap();
        std::fs::write(
            dir.join("embedded/plugins/mine/package.json"),
            b"{\"name\":\"mine\",\"version\":\"0.1.0\"}",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("sessions/--P--/session-abc")).unwrap();
        std::fs::write(
            dir.join("sessions/--P--/session-abc/session.jsonl"),
            b"{\"type\":\"session\"}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("phlpack.json"),
            br#"{
  "formatVersion": 1,
  "pack": {"id":"cli-test","name":"CLI Test","version":"0.1.0"},
  "dsh": {"version":"0.1.2"},
  "runtime": {"kind":"node","nodeVersion":"22"},
  "plugins": [{"id":"mine","version":"0.1.0","source":{"type":"embedded","path":"embedded/plugins/mine"}}],
  "content": {"sessionsIncluded": true, "sessionCount": 1, "secretsExcluded": true}
}"#,
        )
        .unwrap();
    }

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-packcli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn build_validate_inspect_unpack_round_trip() {
        let dir = temp("flow");
        let layout = dir.join("layout");
        std::fs::create_dir_all(&layout).unwrap();
        fixture_layout(&layout);
        let pack = dir.join("built.phlpack");

        cmd_build(&[layout.display().to_string(), pack.display().to_string()]).unwrap();
        assert!(pack.is_file());
        cmd_validate(&[pack.display().to_string()]).unwrap();
        cmd_inspect(&[pack.display().to_string()]).unwrap();

        let out_dir = dir.join("dump");
        cmd_unpack(&[pack.display().to_string(), out_dir.display().to_string()]).unwrap();
        assert!(out_dir.join("embedded/plugins/mine/package.json").is_file());
        assert!(out_dir
            .join("sessions/--P--/session-abc/session.jsonl")
            .is_file());
        // The dest-root confinement kept every written path under the target.
        assert!(out_dir.join("phlpack.json").is_file(), "unpack is verbatim");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_refuses_to_overwrite_and_requires_a_manifest() {
        let dir = temp("guards");
        let layout = dir.join("layout");
        std::fs::create_dir_all(&layout).unwrap();
        fixture_layout(&layout);
        let pack = dir.join("p.phlpack");
        cmd_build(&[layout.display().to_string(), pack.display().to_string()]).unwrap();
        let again = cmd_build(&[layout.display().to_string(), pack.display().to_string()]);
        assert!(again.unwrap_err().contains("已存在"));

        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = cmd_build(&[
            empty.display().to_string(),
            dir.join("e.phlpack").display().to_string(),
        ]);
        assert!(err.unwrap_err().contains("phlpack.json"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpack_refuses_an_invalid_pack_before_writing() {
        let dir = temp("badpack");
        let pack = dir.join("bad.phlpack");
        std::fs::write(&pack, b"not a zip at all").unwrap();
        let out = dir.join("out");
        let err = cmd_unpack(&[pack.display().to_string(), out.display().to_string()]).unwrap_err();
        assert!(!err.is_empty());
        assert!(!out.join("embedded").exists(), "nothing extracted");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

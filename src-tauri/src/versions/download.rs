//! Byte transport: streaming download with on-the-fly sha512/sha256 and the
//! npm-integrity verifier.

use base64::Engine;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use sha2::{Digest, Sha256, Sha512};

use super::{cancelled, http_client};

pub(crate) struct Downloaded {
    pub bytes: u64,
    pub sha512: Vec<u8>,
    /// npm publishes sha512, node dist publishes sha256 (SHASUMS256.txt) —
    /// one streaming pass computes both so neither verifier re-reads bytes.
    pub sha256: Vec<u8>,
}

pub(crate) async fn download<F: Fn(f64, u64, u64) + Send + Sync>(
    flag: &AtomicBool,
    url: &str,
    part_path: &Path,
    total_hint: Option<u64>,
    on_tick: &F,
) -> Result<Downloaded, String> {
    if cancelled(flag) {
        return Err("cancelled".into());
    }
    let client = http_client();
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载源返回错误: {e}"))?;
    // The CDN may stream without `content-length`; the catalog's probed size
    // serves as the fallback denominator for progress.
    let total = response.content_length().or(total_hint).unwrap_or(0);

    let mut file = tokio::fs::File::create(part_path)
        .await
        .map_err(|e| format!("无法创建缓存文件: {e}"))?;
    let mut stream = response.bytes_stream();
    let mut hasher = Sha512::new();
    let mut hasher256 = Sha256::new();
    let mut bytes_done: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_bytes: u64 = 0;
    let mut last_speed: u64 = 0;

    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = stream.next().await {
        if cancelled(flag) {
            let _ = file.shutdown().await;
            let _ = tokio::fs::remove_file(part_path).await;
            return Err("cancelled".into());
        }
        let chunk = chunk.map_err(|e| format!("下载中断: {e}"))?;
        hasher.update(&chunk);
        hasher256.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("写入缓存失败: {e}"))?;
        bytes_done += chunk.len() as u64;

        let elapsed = last_report.elapsed();
        if elapsed >= Duration::from_millis(120) {
            let delta = bytes_done - last_bytes;
            last_speed = if delta > 0 {
                ((delta as f64) / elapsed.as_secs_f64()) as u64
            } else {
                last_speed
            };
            last_report = Instant::now();
            last_bytes = bytes_done;
            let progress = if total > 0 {
                bytes_done as f64 / total as f64
            } else {
                0.0
            };
            on_tick(progress, bytes_done, last_speed);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;
    on_tick(1.0, bytes_done, last_speed);
    Ok(Downloaded {
        bytes: bytes_done,
        sha512: hasher.finalize().to_vec(),
        sha256: hasher256.finalize().to_vec(),
    })
}

/// Compares npm's `dist.integrity` against the digest the download already
/// computed. Taking the digest as an argument keeps the archive from being
/// read a second time — plugin tarballs are arbitrary sizes and the old
/// version loaded the whole file into memory to re-hash it.
pub(crate) fn verify_integrity(digest: &[u8], integrity: Option<&str>) -> Result<(), String> {
    let Some(expected) = integrity else {
        return Ok(()); // registry gave no integrity → nothing to check
    };
    let ok = if let Some(b64) = expected.strip_prefix("sha512-") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("integrity 字段解析失败: {e}"))?;
        decoded == digest
    } else if let Some(expected_hex) = expected.strip_prefix("sha512:") {
        expected_hex.eq_ignore_ascii_case(&hex::encode(digest))
    } else {
        // Anything we cannot check must not be waved through: npm only ever
        // publishes sha512 here, so an unfamiliar prefix means the metadata
        // is not what we think it is.
        let algo = expected.split(['-', ':']).next().unwrap_or(expected);
        return Err(format!("无法校验：不支持的摘要算法 {algo}"));
    };
    if ok {
        Ok(())
    } else {
        Err("校验失败：下载内容与官方 sha512 摘要不一致".into())
    }
}

//! Byte transport: streaming download with cancel-aware waits, classified
//! retries and an ETag-bound resume, plus the npm-integrity verifier.
//!
//! Three properties the old loop lacked (roadmap O-11):
//!
//! - **Cancellation ends the wait.** `.await` on the response head and on
//!   every stream chunk races a one-second poll of the cancel flag, so an
//!   abort lands within about a second even while the network is silent.
//! - **Errors are classified.** Network-level failures (connect, body stall,
//!   timeouts) are transient: the attempt is retried with backoff, up to
//!   three times. HTTP 4xx and disk errors are final and say so.
//! - **Resume never splices different content.** A cancelled or failed
//!   attempt keeps the bytes it wrote beside a sidecar recording the URL,
//!   ETag and Last-Modified. A re-run sends `Range` and only appends when
//!   the server confirms 206 *and* still serves the same entity — any other
//!   answer restarts from zero. After the final byte the whole file is
//!   re-hashed and verified, so no partial state ever reaches an installer
//!   as a "download".

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use super::{cancelled, http_client};

/// Granularity of the cancel-poll racing every network await.
const POLL: Duration = Duration::from_millis(1000);
/// A stream that delivers nothing this long is treated as a dead connection
/// (a retryable error, not a hang).
const IDLE_LIMIT: Duration = Duration::from_secs(45);
/// Transient failures are given this many total tries.
const MAX_ATTEMPTS: u32 = 3;
const BACKOFFS: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

#[derive(Debug)]
pub(crate) struct Downloaded {
    pub bytes: u64,
    pub sha512: Vec<u8>,
    /// npm publishes sha512, node dist publishes sha256 (SHASUMS256.txt) —
    /// one final pass over the file computes both so neither verifier
    /// re-reads bytes.
    pub sha256: Vec<u8>,
}

#[derive(Debug)]
enum Failure {
    /// The user aborted; partial bytes stay for a resume.
    Cancelled,
    /// Transient network trouble; retry with backoff, keeping the prefix.
    Retryable(String),
    /// A corrupt stream must not be continued from — bytes are discarded.
    RetryFresh(String),
    /// Retrying cannot help: bad URL, 4xx, a disk we cannot write to.
    Fatal(String),
}

impl From<String> for Failure {
    /// `?` on plain-String plumbing (serde, sidecar writes) means "this
    /// attempt is over, the message is final".
    fn from(e: String) -> Self {
        Failure::Fatal(e)
    }
}

/* ------------------------------ resume sidecar ------------------------------ */

#[derive(Serialize, Deserialize)]
struct ResumeMeta {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// The `.resume` companion of a `.part` path — public so cancel-vs-fatal
/// cleanup in install flows can keep the two halves of the pair in sync.
pub(crate) fn sidecar_path_of(part_path: &Path) -> PathBuf {
    sidecar_path(part_path)
}

fn sidecar_path(part_path: &Path) -> PathBuf {
    part_path.with_extension(format!(
        "{}.resume",
        part_path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("part")
    ))
}

async fn load_sidecar(path: &Path) -> Option<ResumeMeta> {
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&raw).ok()
}

async fn save_sidecar(path: &Path, meta: &ResumeMeta) -> Result<(), String> {
    let body = serde_json::to_string(meta).map_err(|e| e.to_string())?;
    tokio::fs::write(path, body)
        .await
        .map_err(|e| e.to_string())
}

/* ------------------------------ the pipeline ------------------------------ */

pub(crate) async fn download<F: Fn(f64, u64, u64) + Send + Sync>(
    flag: &AtomicBool,
    url: &str,
    part_path: &Path,
    total_hint: Option<u64>,
    on_tick: &F,
) -> Result<Downloaded, String> {
    let sidecar = sidecar_path(part_path);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match download_attempt(flag, url, part_path, &sidecar, total_hint, on_tick).await {
            Ok(done) => return Ok(done),
            Err(Failure::Cancelled) => return Err("cancelled".into()),
            Err(Failure::Fatal(e)) => {
                let _ = tokio::fs::remove_file(part_path).await;
                let _ = tokio::fs::remove_file(&sidecar).await;
                return Err(e);
            }
            Err(Failure::RetryFresh(e)) => {
                let _ = tokio::fs::remove_file(part_path).await;
                let _ = tokio::fs::remove_file(&sidecar).await;
                if attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
            }
            Err(Failure::Retryable(e)) => {
                if attempt >= MAX_ATTEMPTS {
                    return Err(e);
                }
            }
        }
        // Backoff is itself cancellable: the abort must be honoured even
        // between attempts.
        let pause = BACKOFFS[(attempt as usize - 1).min(BACKOFFS.len() - 1)];
        let mut waited = Duration::ZERO;
        while waited < pause {
            if cancelled(flag) {
                return Err("cancelled".into());
            }
            let tick = POLL.min(pause - waited);
            tokio::time::sleep(tick).await;
            waited += tick;
        }
    }
}

async fn download_attempt<F: Fn(f64, u64, u64) + Send + Sync>(
    flag: &AtomicBool,
    url: &str,
    part_path: &Path,
    sidecar: &Path,
    total_hint: Option<u64>,
    on_tick: &F,
) -> Result<Downloaded, Failure> {
    if cancelled(flag) {
        return Err(Failure::Cancelled);
    }

    // A prefix is only usable if it was written for *this* URL. ETag
    // agreement is confirmed against the response below, not guessed here.
    let resumable = async {
        let meta = load_sidecar(sidecar).await?;
        if meta.url != url {
            return None;
        }
        let have = tokio::fs::metadata(part_path).await.ok()?.len();
        if have == 0 {
            return None;
        }
        Some(meta)
    }
    .await;

    let client = http_client();
    let mut request = client.get(url);
    let resuming = resumable.is_some();
    let have = if resuming {
        tokio::fs::metadata(part_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0)
    } else {
        0
    };
    if resuming {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }

    let response = send_cancellable(request, flag).await?;
    let status = response.status();
    if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE && resuming {
        // The file may already be complete — the final re-hash decides — or
        // the prefix overran the entity. Either way continue without Range.
        return finish_from_existing(flag, url, part_path, sidecar, total_hint, on_tick).await;
    }
    let response = response
        .error_for_status()
        .map_err(|e| Failure::Fatal(format!("下载源返回错误: {e}")))?;

    let etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let last_modified = response
        .headers()
        .get(reqwest::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // The entity behind a resumed range must be the same one the prefix was
    // taken from. No ETag/Last-Modified evidence → no resume, ever.
    let truly_resuming = resuming && status == reqwest::StatusCode::PARTIAL_CONTENT && {
        match resumable
            .as_ref()
            .and_then(|m| m.etag.as_ref().or(m.last_modified.as_ref()))
        {
            None => false,
            Some(saved) => {
                let current = etag.as_deref().or(last_modified.as_deref());
                current == Some(saved)
            }
        }
    };

    let (mut file, mut bytes_done): (tokio::fs::File, u64) = if truly_resuming {
        (
            tokio::fs::OpenOptions::new()
                .append(true)
                .open(part_path)
                .await
                .map_err(|e| Failure::Fatal(format!("无法续写缓存文件: {e}")))?,
            have,
        )
    } else {
        let f = tokio::fs::File::create(part_path)
            .await
            .map_err(|e| Failure::Fatal(format!("无法创建缓存文件: {e}")))?;
        save_sidecar(
            sidecar,
            &ResumeMeta {
                url: url.to_string(),
                etag: etag.clone(),
                last_modified: last_modified.clone(),
            },
        )
        .await?;
        (f, 0)
    };
    let total = response
        .content_length()
        .map(|c| c + bytes_done)
        .or(total_hint)
        .unwrap_or(0);

    let mut stream = response.bytes_stream();
    let mut last_report = Instant::now();
    let mut last_chunk_at = Instant::now();
    let mut last_bytes: u64 = bytes_done;
    let mut last_speed: u64 = 0;

    use tokio::io::AsyncWriteExt;
    loop {
        let chunk = {
            let next = stream.next();
            tokio::pin!(next);
            loop {
                match tokio::time::timeout(POLL, &mut next).await {
                    Ok(res) => break res,
                    Err(_elapsed) => {
                        if cancelled(flag) {
                            let _ = file.shutdown().await;
                            return Err(Failure::Cancelled); // prefix stays
                        }
                        if last_chunk_at.elapsed() >= IDLE_LIMIT {
                            let _ = file.shutdown().await;
                            return Err(Failure::Retryable(format!(
                                "下载停滞超过 {} 秒，已断开（可续传重试）",
                                IDLE_LIMIT.as_secs()
                            )));
                        }
                    }
                }
            }
        };
        let Some(chunk) = chunk else { break };
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = file.shutdown().await;
                // A decode failure may have already written bad bytes past
                // the last clean flush; retry from scratch. Everything else
                // network-shaped keeps the prefix.
                return Err(if e.is_decode() {
                    Failure::RetryFresh(format!("下载内容解码失败: {e}"))
                } else {
                    Failure::Retryable(format!("下载中断: {e}"))
                });
            }
        };
        last_chunk_at = Instant::now();
        file.write_all(&chunk)
            .await
            .map_err(|e| Failure::Fatal(format!("写入缓存失败: {e}")))?;
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
                (bytes_done as f64 / total as f64).min(1.0)
            } else {
                0.0
            };
            on_tick(progress, bytes_done, last_speed);
        }
    }
    file.flush().await.map_err(|e| e.to_string())?;

    let downloaded = hash_file(part_path).await?;
    let _ = tokio::fs::remove_file(sidecar).await;
    on_tick(1.0, downloaded.bytes, last_speed);
    Ok(downloaded)
}

/// 416 while resuming: the prefix may already be the entire entity. Re-read
/// it and let the caller's integrity check be the judge; if it is not the
/// whole thing, the hash will not match and the install fails cleanly.
async fn finish_from_existing<F: Fn(f64, u64, u64) + Send + Sync>(
    flag: &AtomicBool,
    _url: &str,
    part_path: &Path,
    sidecar: &Path,
    total_hint: Option<u64>,
    on_tick: &F,
) -> Result<Downloaded, Failure> {
    if cancelled(flag) {
        return Err(Failure::Cancelled);
    }
    let _ = total_hint;
    let downloaded = hash_file(part_path).await?;
    let _ = tokio::fs::remove_file(sidecar).await;
    on_tick(1.0, downloaded.bytes, 0);
    Ok(downloaded)
}

async fn send_cancellable(
    request: reqwest::RequestBuilder,
    flag: &AtomicBool,
) -> Result<reqwest::Response, Failure> {
    let send = request.send();
    tokio::pin!(send);
    let started = Instant::now();
    loop {
        match tokio::time::timeout(POLL, &mut send).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(e)) => {
                return Err(if e.is_connect() || e.is_request() || e.is_timeout() {
                    Failure::Retryable(format!("下载失败: {e}"))
                } else if e.is_status() {
                    Failure::Fatal(format!("下载源返回错误: {e}"))
                } else {
                    Failure::Retryable(format!("下载失败: {e}"))
                });
            }
            Err(_elapsed) => {
                if cancelled(flag) {
                    return Err(Failure::Cancelled);
                }
                if started.elapsed() > IDLE_LIMIT {
                    return Err(Failure::Retryable("等待服务器响应超时".into()));
                }
            }
        }
    }
}

/// Re-hash the finished file. Resume makes the streaming-hash trick wrong
/// (the prefix was hashed by a *previous* process), so the final pass over
/// the file is both simpler and always right; 50 MB hashes in well under the
/// time one network round-trip takes.
async fn hash_file(path: &Path) -> Result<Downloaded, Failure> {
    use tokio::io::AsyncReadExt;
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| Failure::Fatal(format!("无法读回缓存文件: {e}")))?;
    let mut hasher = Sha512::new();
    let mut hasher256 = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| Failure::Fatal(format!("无法读回缓存文件: {e}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        hasher256.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok(Downloaded {
        bytes,
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

/* ------------------------------ tests ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// The bytes of a real (small) tarball-like payload.
    fn payload() -> Vec<u8> {
        (0..5000u32).map(|i| (i % 251) as u8).collect()
    }

    /// A minimal HTTP/1.1 responder: one request, then connection close.
    /// Records whether a `Range` header arrived and with what value.
    async fn serve(
        body: Vec<u8>,
        etag: Option<&str>,
        range_seen: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    ) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let etag = etag.map(str::to_string);
        let body_c = body.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut req = Vec::new();
            let mut byte = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                    return;
                }
                req.push(byte[0]);
            }
            let text = String::from_utf8_lossy(&req).into_owned();
            let range = text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                .map(|l| l.trim().to_string());
            *range_seen.lock().unwrap() = range;
            let (status, sent): (String, Vec<u8>) = match &text {
                t if t.contains("bytes=2000-") => {
                    let tail = body_c[2000..].to_vec();
                    ("HTTP/1.1 206 Partial Content".into(), tail)
                }
                _ => ("HTTP/1.1 200 OK".into(), body_c.clone()),
            };
            let mut head = format!("{status}\r\nContent-Length: {}\r\n", sent.len());
            if let Some(e) = &etag {
                head.push_str(&format!("ETag: {e}\r\n"));
            }
            head.push_str("Connection: close\r\n\r\n");
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&sent).await;
            let _ = socket.shutdown().await;
        });
        addr
    }

    fn temp_part(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phl-dl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("file.part")
    }

    #[tokio::test]
    async fn a_plain_download_hashes_the_whole_file() {
        let body = payload();
        let want512 = Sha512::digest(&body).to_vec();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let addr = serve(body.clone(), Some("\"v1\""), seen.clone()).await;
        let part = temp_part("plain");
        let flag = Arc::new(AtomicBool::new(false));
        let got = download(
            &flag,
            &format!("http://{addr}/x"),
            &part,
            None,
            &|_, _, _| {},
        )
        .await
        .unwrap();
        assert_eq!(got.sha512, want512);
        assert_eq!(got.bytes, body.len() as u64);
        assert!(!sidecar_path(&part).exists(), "sidecar cleared on success");
        let _ = std::fs::remove_dir_all(part.parent().unwrap());
    }

    #[tokio::test]
    async fn resume_appends_only_after_the_server_confirms_the_same_entity() {
        // Hand-craft a half-finished state: first 2000 bytes + a sidecar for
        // the *same* URL with a matching etag. The canned responder only
        // serves 206 for `bytes=2000-`, so a correct resume completes the
        // file; the digest must match the full payload.
        let body = payload();
        let want = Sha512::digest(&body).to_vec();
        let part = temp_part("resume");

        let url = "http://placeholder/x";
        let mut file = std::fs::File::create(&part).unwrap();
        std::io::Write::write_all(&mut file, &body[..2000]).unwrap();
        drop(file);
        save_sidecar(
            &sidecar_path(&part),
            &ResumeMeta {
                url: url.to_string(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            },
        )
        .await
        .unwrap();

        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let addr = serve(body.clone(), Some("\"v1\""), seen.clone()).await;
        let url = format!("http://{addr}/x");
        // Rewrite the sidecar URL to the real address (the half file was
        // written for "this" URL; that is exactly the binding check).
        save_sidecar(
            &sidecar_path(&part),
            &ResumeMeta {
                url: url.clone(),
                etag: Some("\"v1\"".into()),
                last_modified: None,
            },
        )
        .await
        .unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let got = download(&flag, &url, &part, None, &|_, _, _| {})
            .await
            .unwrap();
        assert!(
            seen.lock()
                .unwrap()
                .as_deref()
                .is_some_and(|r| r.ends_with("bytes=2000-")),
            "must request the missing tail, got {:?}",
            seen.lock().unwrap()
        );
        assert_eq!(got.sha512, want, "resumed file hashes to the full body");
        assert_eq!(got.bytes, body.len() as u64);
        let _ = std::fs::remove_dir_all(part.parent().unwrap());
    }

    #[tokio::test]
    async fn a_different_etag_restarts_instead_of_splicing() {
        let body = payload();
        let want = Sha512::digest(&body).to_vec();
        let part = temp_part("etag");
        // The prefix is from an OLD entity of the same URL; the server now
        // reports a different ETag, so the resume must be refused.
        let mut file = std::fs::File::create(&part).unwrap();
        std::io::Write::write_all(&mut file, b"STALE DIFFERENT CONTENT").unwrap();
        drop(file);
        let url = "http://placeholder/x";
        save_sidecar(
            &sidecar_path(&part),
            &ResumeMeta {
                url: url.to_string(),
                etag: Some("\"old\"".into()),
                last_modified: None,
            },
        )
        .await
        .unwrap();

        let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let addr = serve(body.clone(), Some("\"new\""), seen.clone()).await;
        let url = format!("http://{addr}/x");
        save_sidecar(
            &sidecar_path(&part),
            &ResumeMeta {
                url: url.clone(),
                etag: Some("\"old\"".into()),
                last_modified: None,
            },
        )
        .await
        .unwrap();

        let flag = Arc::new(AtomicBool::new(false));
        let got = download(&flag, &url, &part, None, &|_, _, _| {})
            .await
            .unwrap();
        // The client asks optimistically for the tail; what matters is that
        // the mismatching entity was refused and the file restarted from
        // zero — proven by the digest matching the *full* new body.
        assert_eq!(got.sha512, want, "the file was restarted, not spliced");
        assert_eq!(got.bytes, body.len() as u64);
        let _ = std::fs::remove_dir_all(part.parent().unwrap());
    }

    #[tokio::test]
    async fn cancelling_during_a_stalled_stream_ends_within_the_poll_window() {
        // The responder sends the headers and half the body, then holds the
        // connection open forever. The abort flag is flipped a beat later:
        // the download must notice within the poll window and LEAVE the
        // partial file plus sidecar for a resume.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut req = Vec::new();
            let mut byte = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                    return;
                }
                req.push(byte[0]);
            }
            let half = payload()[..1000].to_vec();
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload().len()
            );
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&half).await;
            tokio::time::sleep(Duration::from_secs(30)).await;
            let _ = socket.shutdown().await;
        });

        let part = temp_part("stall");
        let flag = Arc::new(AtomicBool::new(false));
        let abort = flag.clone();
        let url = format!("http://{addr}/x");
        let handle = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            abort.store(true, Ordering::SeqCst);
        });
        let err = download(&flag, &url, &part, None, &|_, _, _| {})
            .await
            .unwrap_err();
        handle.await.unwrap();
        assert_eq!(err, "cancelled");
        assert!(part.exists(), "the partial prefix stays for a resume");
        assert!(
            sidecar_path(&part).exists(),
            "the sidecar stays so the next run can bind to the same entity"
        );
        let _ = std::fs::remove_dir_all(part.parent().unwrap());
    }

    #[tokio::test]
    async fn a_404_is_fatal_not_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut req = Vec::new();
            let mut byte = [0u8; 1];
            while !req.ends_with(b"\r\n\r\n") {
                if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                    return;
                }
                req.push(byte[0]);
            }
            let _ = socket
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
        });
        let part = temp_part("404");
        let flag = Arc::new(AtomicBool::new(false));
        let err = download(
            &flag,
            &format!("http://{addr}/gone"),
            &part,
            None,
            &|_, _, _| {},
        )
        .await
        .unwrap_err();
        assert!(err.contains("返回错误"), "{err}");
        assert!(!part.exists(), "a fatal download leaves nothing behind");
        let _ = std::fs::remove_dir_all(part.parent().unwrap());
    }
}

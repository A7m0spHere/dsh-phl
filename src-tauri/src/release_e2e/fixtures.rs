//! Headless E2E fixtures for the Windows release gate.
//!
//! These helpers drive the real `*_inner` Rust pipeline against a throwaway
//! data root — no Tauri window, no IPC, no GUI. They exist so a clean
//! `cargo test --workspace -- --ignored release_e2e` can prove the same
//! install → download → create → launch → stop → adopt → snapshot → pack →
//! migrate → update paths a user hits after installing PHL.
//!
//! Three constraints shape everything here:
//!   * The crate's internals are `pub(crate)`, so this module lives inside the
//!     crate (mounted from `lib.rs` under `#[cfg(all(test, windows))]`) and
//!     calls them directly — `tauri::test`'s MockRuntime does not load on this
//!     toolchain (`ipc_contract.rs:11-17`), so an in-app invoke round-trip is
//!     not available.
//!   * Every download and node/runtime fetch goes through a local mock HTTP
//!     server; nothing here touches the network. The DSH tarball carries a
//!     dependency-free `package.json` so `run_install` skips the real npm step
//!     (`dependencies.rs:21`); the Node archive wraps the host's *real*
//!     `node.exe` because the runtime health gate executes `node --version`
//!     and demands an exact match (`runtimes/install.rs:196-226`).
//!   * `kill_on_drop` is not set anywhere, so dropping a `tokio::process::Child`
//!     detaches rather than terminates — every journey ends by killing what it
//!     spawned (see `journeys::Session::cleanup`).

#![allow(dead_code)] // the four fixture lanes (journeys, faults, F2, F5) use overlapping subsets

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A fresh, absolute scratch directory named after a tag and the test process.
/// Mirrors the `temp_root` pattern already used across the suite
/// (`paths.rs:366`, `registry.rs:605`, …).
pub(crate) fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("phl-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// An absolute path string without a trailing separator — `validate_root`
/// rejects relative paths and the pointer file stores the string verbatim.
pub(crate) fn abs(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .to_string()
}

/// A cancellable flag, false unless the test flips it.
pub(crate) fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// A `Task` handle for installers that need one (`run_install`,
/// `run_runtime_install`). Same shape as `versions/mod.rs` `test_task`.
pub(crate) fn task(kind: &'static str, id: &str) -> crate::resources::Task {
    crate::resources::Tasks::default()
        .begin(
            crate::resources::TaskInfo::new(id.into(), kind, format!("e2e-{id}"), &[]),
            None,
        )
        .unwrap()
}

/* --------------------------- mock HTTP server --------------------------- */

/// How the server answers one request path. Unknown paths get a 404. Each
/// connection serves exactly one request and closes (`Connection: close`),
/// like the canned responders in `versions/download.rs:496`.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) status: u16,
    pub(crate) etag: Option<String>,
    pub(crate) body: Vec<u8>,
    /// Serve `Range: bytes=N-` requests with a 206 of the tail (and record
    /// the header in `seen_ranges`) — the resume path of `download()`.
    pub(crate) range: bool,
    /// Write only this many body bytes, then drop the connection with the
    /// declared Content-Length still unmet: a real mid-stream abort.
    /// `usize::MAX` disables truncation.
    pub(crate) truncate_at: usize,
    /// Write this many body bytes, then hold the connection silent (no more
    /// bytes, no close) for `stall_hold`: the transfer's cancel poll is
    /// meant to observe an abort mid-silence. `usize::MAX` disables.
    pub(crate) stall_after: usize,
    pub(crate) stall_hold: std::time::Duration,
}

impl Route {
    pub(crate) fn bytes(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            etag: None,
            body,
            range: false,
            truncate_at: usize::MAX,
            stall_after: usize::MAX,
            stall_hold: std::time::Duration::from_secs(6),
        }
    }
    pub(crate) fn text(body: &str) -> Self {
        Self::bytes(body.as_bytes().to_vec())
    }
    pub(crate) fn with_etag(mut self, etag: &str) -> Self {
        self.etag = Some(etag.to_string());
        self
    }
    pub(crate) fn with_range(mut self) -> Self {
        self.range = true;
        self
    }
    pub(crate) fn truncated_at(mut self, n: usize) -> Self {
        self.truncate_at = n;
        self
    }
    pub(crate) fn stalling_after(mut self, n: usize, hold: std::time::Duration) -> Self {
        self.stall_after = n;
        self.stall_hold = hold;
        self
    }
}

/// A minimal HTTP/1.1 responder on an ephemeral port, backed by a mutable
/// route table so a test can flip behaviour between rounds (e.g. make the
/// *second* download of the same URL abort mid-stream).
pub(crate) struct MockServer {
    addr: SocketAddr,
    routes: Arc<Mutex<HashMap<String, Route>>>,
    /// `Range` headers the server answered, in arrival order.
    pub(crate) seen_ranges: Arc<Mutex<Vec<String>>>,
}

impl MockServer {
    pub(crate) async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Arc<Mutex<HashMap<String, Route>>> = Arc::new(Mutex::new(HashMap::new()));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (routes_loop, seen_loop) = (routes.clone(), seen.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let routes_h = routes_loop.clone();
                let seen_h = seen_loop.clone();
                tokio::spawn(async move {
                    let mut req = Vec::new();
                    let mut b = [0u8; 1];
                    while !req.ends_with(b"\r\n\r\n") {
                        match socket.read(&mut b).await {
                            Ok(0) | Err(_) => return,
                            Ok(_) => req.push(b[0]),
                        }
                    }
                    let text = String::from_utf8_lossy(&req).into_owned();
                    let path = text
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let range_hdr = text
                        .lines()
                        .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                        .map(|l| l.trim().to_string());
                    let route = routes_h.lock().unwrap().get(&path).cloned();
                    let Some(route) = route else {
                        respond(&mut socket, 404, None, b"not found", usize::MAX).await;
                        return;
                    };
                    if range_hdr.is_some() && route.range {
                        if let Some(hdr) = &range_hdr {
                            seen_h.lock().unwrap().push(hdr.clone());
                        }
                        let from = range_start(range_hdr.as_deref().unwrap_or("bytes=0-"));
                        let tail = route
                            .body
                            .get(from..)
                            .map(<[u8]>::to_vec)
                            .unwrap_or_default();
                        respond_partial(&mut socket, &tail, route.etag.as_deref()).await;
                        return;
                    }
                    if route.stall_after != usize::MAX {
                        respond_stalled(&mut socket, &route).await;
                        return;
                    }
                    respond(
                        &mut socket,
                        route.status,
                        route.etag.as_deref(),
                        &route.body,
                        route.truncate_at,
                    )
                    .await;
                });
            }
        });
        Self {
            addr,
            routes,
            seen_ranges: seen,
        }
    }

    /// The URL prefix for `registry_base` / `dist_base` style callers.
    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub(crate) fn insert(&self, path: &str, route: Route) {
        self.routes.lock().unwrap().insert(path.to_string(), route);
    }

    pub(crate) fn clear_ranges(&self) {
        self.seen_ranges.lock().unwrap().clear();
    }
}

async fn respond(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    etag: Option<&str>,
    body: &[u8],
    truncate_at: usize,
) {
    let declared = body.len();
    let writing = truncate_at.min(declared);
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Length: {declared}\r\n",
        reason(status)
    );
    if let Some(e) = etag {
        head.push_str(&format!("ETag: {e}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&body[..writing]).await;
    let _ = socket.shutdown().await;
}

async fn respond_partial(socket: &mut tokio::net::TcpStream, body: &[u8], etag: Option<&str>) {
    let mut head = format!(
        "HTTP/1.1 206 {}\r\nContent-Length: {}\r\n",
        reason(206),
        body.len()
    );
    if let Some(e) = etag {
        head.push_str(&format!("ETag: {e}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.shutdown().await;
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        _ => "Error",
    }
}

/// `bytes=N-` → N (the only range shape `download()` sends).
fn range_start(hdr: &str) -> usize {
    hdr.rfind('=')
        .map(|i| &hdr[i + 1..])
        .unwrap_or(hdr)
        .split('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/* -------------------------- archive builders --------------------------- */

/// A gzipped npm-style tarball (`package/…`) for the fake DSH version. The
/// payload is dependency-free so `run_install` skips the real npm step
/// (`package_requires_deps`, `dependencies.rs:21`); `lib/bin.js` is the web
/// entrypoint the launcher runs. Same build sequence as
/// `make_test_tarball` (`versions/mod.rs:449`).
pub(crate) fn fake_dsh_tarball(version: &str) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar = tar::Builder::new(&mut enc);
        let pkg = format!(
            "{{\"name\":\"@deepseek-ai/dsh\",\"version\":\"{version}\",\"bin\":{{\"dsh\":\"lib/bin.js\"}}}}"
        );
        append_bytes(&mut tar, "package/package.json", pkg.as_bytes(), 0o644);
        append_bytes(
            &mut tar,
            "package/lib/bin.js",
            fake_dsh_bin_js().as_bytes(),
            0o755,
        );
        append_bytes(
            &mut tar,
            "package/README.md",
            b"fake DSH shipped by the release E2E gate\n",
            0o644,
        );
        // Dropping the builder pads the archive; into_inner would double-write.
        drop(tar);
    }
    enc.finish().unwrap()
}

fn append_bytes<B: std::io::Write>(tar: &mut tar::Builder<B>, name: &str, bytes: &[u8], mode: u32) {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mtime(1);
    header.set_cksum();
    tar.append_data(
        &mut header,
        Path::new(name),
        std::io::Cursor::new(bytes.to_vec()),
    )
    .unwrap();
}

/// The DSH entrypoint the fake tarball ships: parses `--port` from argv,
/// binds 127.0.0.1, prints the token line (`dsh web:` marker,
/// `process.rs:102`) then serves so the TCP readiness connect succeeds.
fn fake_dsh_bin_js() -> &'static str {
    r#"// Fake DSH web entrypoint shipped by the release E2E gate.
const http = require('http');
let port = 3000;
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  if (argv[i] === '--port' && argv[i + 1]) { port = Number(argv[i + 1]); break; }
}
process.stdout.write('dsh web: http://127.0.0.1:' + port + '/?token=e2e\n');
const server = http.createServer((_req, res) => { res.writeHead(200); res.end('ok'); });
server.listen(port, '127.0.0.1');
"#
}

/// sha512 in the `sha512-<base64>` form `verify_integrity` accepts
/// (`download.rs:454`).
pub(crate) fn integrity_sha512(bytes: &[u8]) -> String {
    use base64::Engine as _;
    use sha2::{Digest as _, Sha512};
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes))
    )
}

/* ------------------------------ node probe ----------------------------- */

/// The system `node`: its path, its exact reported version and major. The
/// runtime health gate runs the installed binary and demands the exact
/// reported version, so the fake Node archive must carry the *real*
/// `node.exe` and be named after the version it reports.
pub(crate) struct NodeInfo {
    pub(crate) exe: PathBuf,
    /// Full version without the leading `v` (semver form).
    pub(crate) version: String,
    pub(crate) major: u32,
}

impl NodeInfo {
    pub(crate) fn discover() -> Option<Self> {
        let probe = |args: &[&str]| -> Option<String> {
            let out = std::process::Command::new("node")
                .args(args)
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
        };
        let reported = probe(&["--version"])?; // "v22.x.y"
        let version = reported.strip_prefix('v')?.to_string();
        let major: u32 = version.split('.').next()?.parse().ok()?;
        let exe = PathBuf::from(probe(&["-e", "process.stdout.write(process.execPath)"])?);
        if !exe.exists() {
            return None;
        }
        Some(Self {
            exe,
            version,
            major,
        })
    }

    /// The `run_runtime_install` version-name conventions: `node-<major>`
    /// for the install dir, the full semver for `version`.
    pub(crate) fn runtime_name(&self) -> String {
        format!("node-{}", self.major)
    }
}

/// The Node dist archive filename for this platform, as `run_runtime_install`
/// derives it (`dist_platform`, `install.rs:230`): `node-v{ver}-win-x64.zip`.
pub(crate) fn node_dist_filename(version: &str) -> String {
    format!("node-v{version}-win-x64.zip")
}

/// A fake Node `win-x64` dist zip whose payload is the real `node.exe` at the
/// layout `extract_zip` expects (one top directory, stripped on unpack), plus
/// the SHASUMS256.txt line the installer verifies against
/// (`runtimes/install.rs:101-102`). Returns (zip bytes, shasums body).
pub(crate) fn fake_node_dist(node: &NodeInfo) -> (Vec<u8>, String) {
    let filename = node_dist_filename(&node.version);
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        // The zip's single top-level directory is whatever precedes the first
        // slash — `extract_zip`'s `strip_first` drops it, so the entry below
        // lands at staging/node.exe, exactly where the Windows health gate
        // runs it (`check_runtime_health`, `install.rs:196`).
        zip.start_file(format!("node-v{}/node.exe", node.version), opts)
            .unwrap();
        zip.write_all(&std::fs::read(&node.exe).unwrap()).unwrap();
        zip.finish().unwrap();
    }
    use sha2::{Digest as _, Sha256};
    let sha = hex::encode(Sha256::digest(&buf));
    (buf, format!("{sha}  {filename}\n"))
}

/// Serve the first `stall_after` body bytes, then hold the connection silent
/// (no close) until `stall_hold` elapses. While held, the download's 1-second
/// cancel poll is the only thing that can end the transfer — exactly the
/// stalled-connection shape O-11 promises a cancel can interrupt.
async fn respond_stalled(socket: &mut tokio::net::TcpStream, route: &Route) {
    let declared = route.body.len();
    let cut = route.stall_after.min(declared);
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {declared}\r\n",
        route.status,
        reason(route.status)
    );
    if let Some(e) = &route.etag {
        head.push_str(&format!("ETag: {e}\r\n"));
    }
    head.push_str("Connection: keep-alive\r\n\r\n");
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&route.body[..cut]).await;
    tokio::time::sleep(route.stall_hold).await;
    let _ = socket.shutdown().await;
}

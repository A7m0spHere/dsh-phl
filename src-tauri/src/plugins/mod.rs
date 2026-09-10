//! Plugin Market backing: the DSH community catalog (awesome-dsh-plugin
//! `plugins.json`) plus a real download / verify / install pipeline.
//!
//! Source priority follows the ecosystem convention: a repo-verified npm
//! package beats an author-built tarball, which beats pulling the GitHub
//! repo source. Installation lands in the instance profile's `node_modules`
//! and registers the plugin in `cordis.patch.yml` — the same files DSH's own
//! `dsh plugin add` manages — so PHL never invents a private layout.
//!
//! Split along its seams (Batch F): `catalog` (the community registry),
//! `resolve` (source → tarball URL), `security` (trust levels + GitHub
//! commit pinning), `install` (the staging/swap pipeline) and `cordis`
//! (line-level cordis.patch.yml editing).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

pub(crate) mod catalog;
pub(crate) mod cordis;
pub(crate) mod install;
pub(crate) mod resolve;
pub(crate) mod security;

pub(crate) use cordis::disabled_plugin_ids;
pub(crate) use resolve::sanitize_pkg_path;
pub(crate) use security::compute_trust;

pub(crate) const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// The aggregated `plugins.json` is ~2.5 MB and the main site can take 40 s+
/// on a slow link — the default 15 s aborts mid-transfer, which is exactly
/// the "插件市场加载失败" report. Catalog fetches get their own ceiling.
pub(crate) const REGISTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Catalog sources, tried in order. The GitHub Pages site carries the same
/// aggregated `plugins.json` as the main domain (CI-built) and answers fast
/// and completely where the main site has been observed to stall mid-
/// transfer. `dsh-ai.org` serves the community's byte-compatible copy and
/// sits last: its refresh lags badly (2026-09 check: 1837 entries / 2026-08-21
/// against 3196 / 2026-09-05 upstream), so it must never shadow a fresh
/// source — the catalog reply says which base served it. The jsDelivr/raw
/// entries stay dropped on purpose: the repo's default branch only carries
/// per-plugin YAML sources, never the aggregated JSON.
pub(crate) const REGISTRY_FALLBACKS: &[&str] = &[
    "https://awesome-dsh-plugin.com",
    "https://awesome-dsh-plugin.github.io/awesome-dsh-plugin",
    "https://dsh-ai.org",
];

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("PHL/0.1 (dsh-phl)")
        .connect_timeout(HTTP_TIMEOUT)
        .build()
        .expect("reqwest client")
}

/* ----------------------------- wire types ----------------------------- */

/// Tagged exactly like the frontend `PluginSource` union.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PluginSourceWire {
    Npm {
        pkg: String,
    },
    Tarball {
        url: String,
        integrity: Option<String>,
    },
    Github {
        repo: String,
    },
}

/// Mirrors the frontend `Plugin`. Live registry entries carry no releases —
/// the concrete version resolves against npm at install time.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMeta {
    pub id: String,
    pub name: String,
    pub author: String,
    pub category: String,
    pub summary: String,
    pub summary_en: Option<String>,
    pub repo_url: Option<String>,
    pub screenshots: Vec<String>,
    pub source: PluginSourceWire,
    pub official: bool,
    pub downloads: u64,
    pub stars: Option<u64>,
    pub added_at: Option<String>,
    pub releases: Vec<PluginReleaseMeta>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginReleaseMeta {
    pub version: String,
    pub published_at: String,
    pub source: Option<PluginSourceWire>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "stage")]
pub enum PluginProgressEvent {
    Preparing,
    #[serde(rename_all = "camelCase")]
    Downloading {
        progress: f64,
        bytes_done: u64,
        bytes_per_sec: u64,
    },
    Verifying,
    #[serde(rename_all = "camelCase")]
    Installing {
        progress: f64,
    },
    /// pnpm is materialising the plugin's dependency closure — minutes-long
    /// on a cold cache, with no machine-readable progress (npm/pnpm only draw
    /// their bar on a TTY). The card must go indeterminate, not freeze at the
    /// last extraction percentage.
    InstallingDeps,
    /// The transaction commit: marker, Cordis registration, enable flag.
    Committing,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallOutcome {
    pub version: String,
    /// The id written into `cordis.patch.yml` (npm package name or repo name).
    pub registry_id: String,
    /// verified | pinned | unverified — from install-time facts (T-107).
    pub trust: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginVersionInfo {
    pub version: Option<String>,
}

/// One plugin-catalog reply: the entries *and* the provenance of this exact
/// fetch. The market UI surfaces it so a fallback to a slower-stale mirror —
/// or an offline cache replay — can never be mistaken for the live catalog.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogWire {
    /// The base URL that served the catalog, or `"cache"` for an offline replay.
    pub served_from: String,
    /// True when the source the user configured failed and a fallback served this.
    pub used_fallback: bool,
    /// True when every online source failed and the on-disk copy was replayed.
    pub from_cache: bool,
    /// The registry's own `updated` stamp (YYYY-MM-DD), when present.
    pub updated: Option<String>,
    pub plugins: Vec<PluginMeta>,
}

pub(crate) fn source_kind(source: &PluginSourceWire) -> &'static str {
    match source {
        PluginSourceWire::Npm { .. } => "npm",
        PluginSourceWire::Tarball { .. } => "tarball",
        PluginSourceWire::Github { .. } => "github",
    }
}

fn sanitize_cache_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn cancelled(flag: &AtomicBool) -> bool {
    flag.load(Ordering::SeqCst)
}

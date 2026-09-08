//! The `.phlpack` pipeline, host half (development spec part 5 & 6, P2-1).
//!
//! The format itself — schema, validation, integrity, ZIP building and the
//! confined extractor — moved to the host-free `phl-pack-core` crate (spec
//! §15), shared with the `phl-pack` CLI. What remains here is the PHL glue:
//!
//! - [`export`] — turn a *managed instance* into a pack (scans, plugin
//!   classification, credential stripping, the privacy gate);
//! - [`install`] — resolve a pack's dependencies and materialise one as a new
//!   instance (staging, manifest write, rollback).
//!
//! Both are thin over the core's [`read_pack_from_path`] / [`PackBuilder`] /
//! [`unpack`] primitives. This module re-exports the core surface under the
//! historical `crate::pack::…` paths so the host call sites (and their tests)
//! keep reading one namespace; the `[state]` error coding that commands
//! require is added here, at the host boundary ([`pack_error`]) — the core
//! never speaks in transport codes.

pub(crate) mod export;
pub(crate) mod install;

// Historical path compatibility: `super::format::…` / `super::write::…` are
// used by the glue modules; re-exporting the core's modules keeps them valid.
pub use phl_pack_core::format;
pub use phl_pack_core::unpack;
pub use phl_pack_core::write;

// The full historical surface; items the glue modules do not touch today
// (integrity helpers, ceilings) stay re-exported deliberately — they are the
// compatibility namespace the format modules and their tests are written
// against, and the pack pipeline's error vocabulary (`PackError`).
#[allow(unused_imports)]
pub use phl_pack_core::{
    read_pack, read_pack_from_path, read_pack_from_path_with_cancel, read_pack_with_cancel,
    sha256_hex, validate_manifest, EnvironmentSection, PackContent, PackDsh, PackError,
    PackIntegrity, PackMeta, PackPlugin, PackPrivacy, PackRuntime, PhlPackManifest, PluginSource,
    ValidatedPack, MANIFEST_NAME, MAX_ENTRIES, MAX_MANIFEST_BYTES, MAX_SINGLE_ENTRY,
    MAX_UNCOMPRESSED_TOTAL, PACK_FORMAT_VERSION,
};
// `is_secret_entry_name` is the §12 predicate export.rs reuses to pre-warn at
// preview time (`add_tree`'s `TreeAdd` result flows through without being
// named at the host). `PackBuilder` itself comes in via `super::write`.
pub use phl_pack_core::write::is_secret_entry_name;

/// The command-boundary wrapper: core validation errors surface to the UI as
/// stable `[state]`-coded strings (they are all "this pack is not acceptable"
/// refusals, not transport failures).
pub(crate) fn pack_error(e: PackError) -> String {
    if e == PackError::Cancelled {
        "cancelled".into()
    } else {
        crate::errors::coded(crate::errors::ErrCode::State, e.detail())
    }
}

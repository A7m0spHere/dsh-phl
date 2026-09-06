//! Stable machine-readable error codes for the string-typed IPC surface.
//!
//! Commands return `Result<_, String>`, and the frontend renders those
//! strings directly. A user (and a regression log) cannot act on a wall of
//! prose — the roadmap's ask (O-09) was stable codes, phase, retryability
//! and a next action. Encoding: the Rust side prefixes messages with
//! `[code] `; the frontend [`parse`] helper strips it, marks retryable
//! failures, and leaves the remaining human text untouched. Un-prefixed
//! messages (the vast legacy surface, and `"cancelled"` by contract) parse
//! to `code: None` and display as before — adoption is per-site, not big-bang.
//!
//!   - Cancellation is deliberately *not* coded: `Err("cancelled")` is the
//!     sentinel every transfer loop and the task registry compare against.
//!   - Codes classify the *cause*, the message keeps the specifics:
//!     `[not-found] 实例不存在: main` — the user sees the sentence, the log
//!     and the UI logic get the kind.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrCode {
    /// The referenced instance / version / snapshot / runtime is not there.
    NotFound,
    /// Another operation holds the resource (roadmap O-05's conflict).
    Busy,
    /// The chosen port is taken; pick another or let auto-port run.
    PortConflict,
    /// Network-level failure that backoff can plausibly fix.
    NetRetryable,
    /// Network-level failure that will not fix itself (bad URL, 4xx).
    NetFatal,
    /// The OS denied a file operation (ACLs, in use, elevation).
    Permission,
    /// Target volume out of space.
    DiskFull,
    /// On-disk state disagrees with what the operation assumed (open
    /// migration journal, unreadable manifest, unparseable metadata).
    State,
}

impl ErrCode {
    pub fn tag(self) -> &'static str {
        match self {
            ErrCode::NotFound => "not-found",
            ErrCode::Busy => "busy",
            ErrCode::PortConflict => "port-conflict",
            ErrCode::NetRetryable => "net-retryable",
            ErrCode::NetFatal => "net-fatal",
            ErrCode::Permission => "permission",
            ErrCode::DiskFull => "disk-full",
            ErrCode::State => "state",
        }
    }

    /// A retry can plausibly change the outcome (transient network trouble,
    /// a resource that another task may release). A stale conflict (`state`)
    /// and identity failures need a human step first.
    ///
    /// Mirrored in `src/lib/errorCodes.ts` for the UI; the Rust callers that
    /// decide retry affordances arrive later (task centre actions), hence
    /// the allowance until then.
    #[allow(dead_code)]
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrCode::NetRetryable | ErrCode::Busy | ErrCode::PortConflict
        )
    }

    /// One-line next-action wording for the UI, per code (the shipped text
    /// is the frontend mirror; kept here so both sides are anchored).
    #[allow(dead_code)]
    pub fn hint(self) -> &'static str {
        match self {
            ErrCode::NotFound => "对象不存在，刷新列表或检查拼写。",
            ErrCode::Busy => "等另一个操作结束后重试；可在任务中心查看进度。",
            ErrCode::PortConflict => "端口被占用：改用自动端口，或释放该端口后重试。",
            ErrCode::NetRetryable => "网络波动，通常直接重试即可。",
            ErrCode::NetFatal => "地址或来源有问题，检查下载源设置。",
            ErrCode::Permission => "系统拒绝了文件操作：检查目录权限或占用后重试。",
            ErrCode::DiskFull => "磁盘空间不足，清理后重试。",
            ErrCode::State => "磁盘状态与操作前提不符，需要先处理遗留记录。",
        }
    }
}

/// Prefix a message with its code.
pub fn coded(code: ErrCode, message: impl std::fmt::Display) -> String {
    format!("[{}] {message}", code.tag())
}

/// Best-effort classification of a filesystem `io::Error` into a code.
pub fn io_code(e: &std::io::Error) -> ErrCode {
    #[derive(PartialEq)]
    enum Kind {
        NotFound,
        Perm,
        Full,
        Other,
    }
    use std::io::ErrorKind as K;
    let kind = match e.kind() {
        K::NotFound => Kind::NotFound,
        K::PermissionDenied => Kind::Perm,
        K::StorageFull => Kind::Full,
        // Windows reports "not enough space" as an os error, not StorageFull.
        _ if e.raw_os_error() == Some(112) => Kind::Full,
        _ => Kind::Other,
    };
    match kind {
        Kind::NotFound => ErrCode::NotFound,
        Kind::Perm => ErrCode::Permission,
        Kind::Full => ErrCode::DiskFull,
        Kind::Other => ErrCode::State,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_prefix_messages_and_classify_retryability() {
        let msg = coded(ErrCode::Busy, "实例 main 正被另一个操作占用");
        assert!(msg.starts_with("[busy] "), "{msg}");
        assert!(ErrCode::Busy.retryable());
        assert!(!ErrCode::NotFound.retryable());
        assert!(ErrCode::NetRetryable.hint().contains("重试"));
    }

    #[test]
    fn io_kinds_map_to_specific_codes() {
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(io_code(&e), ErrCode::Permission);
        let e = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert_eq!(io_code(&e), ErrCode::NotFound);
    }
}

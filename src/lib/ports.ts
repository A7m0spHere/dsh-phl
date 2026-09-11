/**
 * The smallest port a DSH WebUI can be opened on. Chromium's reserved-port
 * blocklist refuses to *navigate* to the system/service ports below 1024
 * (`ERR_UNSAFE_PORT`) even though binding them succeeds on Windows — so these
 * ports must never reach a launch, a stored manifest, or a WebUI window.
 *
 * Mirror of `MIN_WEB_PORT` in `src-tauri/src/launch/process.rs`; change the
 * two together. (History: the adoption draft persisted `port: 0`, the auto
 * scan advanced 0 → 1, and every "打开" on that instance died on
 * ERR_UNSAFE_PORT — 2026-09-11.)
 */
export const MIN_WEB_PORT = 1024

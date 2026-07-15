//! Detects when the server has redeployed a newer build than the one this tab
//! loaded, and reloads the tab. The server embeds a build hash into the SSR
//! shell (`AppState::asset_version`, `src/server/mod.rs`) and pushes the same
//! value as the first event on every `watch`/`watch-multi` SSE connection
//! (`src/server/api.rs`) — a rolling deploy always drops and reconnects those
//! connections, so this fires within one SSE reconnect of a redeploy.

#[cfg(target_arch = "wasm32")]
static BASELINE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Capture the build this tab loaded, from the SSR shell's meta tag. Call once,
/// on hydrate, before any SSE connection opens.
#[cfg(target_arch = "wasm32")]
pub fn init_baseline() {
    let value = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| {
            d.query_selector("meta[name=\"roder-asset-version\"]")
                .ok()
                .flatten()
        })
        .and_then(|el| el.get_attribute("content"))
        .unwrap_or_default();
    let _ = BASELINE.set(value);
}

#[cfg(not(target_arch = "wasm32"))]
pub fn init_baseline() {}

/// True if `server_version` indicates the server is running a different build
/// than the one this tab loaded with. An empty `baseline` or `server_version`
/// never counts as stale — fail open rather than reload-looping on a bad read.
/// Only reachable from `on_server_version`'s wasm32 body — cfg-gated the same
/// way (plus `test`, so the unit tests below can exercise it directly) so it
/// isn't flagged as dead code on a native, non-test build.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn is_stale(baseline: &str, server_version: &str) -> bool {
    !baseline.is_empty() && !server_version.is_empty() && baseline != server_version
}

/// Called with the payload of a `version` SSE event. Reloads the tab exactly
/// once the server's build no longer matches the baseline captured at load.
#[cfg(target_arch = "wasm32")]
pub fn on_server_version(server_version: &str) {
    let Some(baseline) = BASELINE.get() else {
        return;
    };
    if !is_stale(baseline, server_version) {
        return;
    }
    if let Some(win) = web_sys::window() {
        let _ = win.location().reload();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn on_server_version(_server_version: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_when_versions_differ() {
        assert!(is_stale("aaa", "bbb"));
    }

    #[test]
    fn not_stale_when_versions_match() {
        assert!(!is_stale("aaa", "aaa"));
    }

    #[test]
    fn not_stale_when_baseline_empty() {
        assert!(!is_stale("", "bbb"));
    }

    #[test]
    fn not_stale_when_server_version_empty() {
        assert!(!is_stale("aaa", ""));
    }
}

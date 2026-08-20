//! Drives a real headless Chromium (via `rustwright`) at a live `roder`
//! instance and fails on any console error or failed network request.
//!
//! `wait_until("networkidle")` never fires for this app — the dashboard
//! keeps a live `/api/watch` connection open, so CDP's "no in-flight
//! requests for 500ms" condition is never met. `"load"` plus a fixed settle
//! delay is used instead.
//!
//! Needs `RODER_E2E_BASE_URL` (and a `CHROME`/`CHROMIUM`/`RUSTWRIGHT_CHROMIUM`
//! pointing at a real browser binary) to point at a running instance;
//! skipped otherwise so a plain `cargo test` stays hermetic.

use std::time::Duration;

use rustwright::{chromium, GotoOptions, LaunchOptions};

#[test]
fn app_loads_without_console_errors_or_failed_requests() {
    let Ok(base_url) = std::env::var("RODER_E2E_BASE_URL") else {
        eprintln!("RODER_E2E_BASE_URL not set — skipping browser smoke test");
        return;
    };

    let browser = chromium()
        .launch(LaunchOptions::default().headless(true))
        .expect("failed to launch chromium");

    let result = (|| -> Result<(), String> {
        let page = browser.new_page().map_err(|e| e.to_string())?;
        page.goto(
            &base_url,
            GotoOptions::default().wait_until("load").timeout(30_000.0),
        )
        .map_err(|e| format!("navigation to {base_url} failed: {e}"))?;

        // Let post-load hydration and its follow-up fetches (catalog,
        // overview, the initial watch subscription, ...) settle.
        std::thread::sleep(Duration::from_millis(2000));

        let console = page
            .console_records(false, false)
            .map_err(|e| e.to_string())?;
        let errors: Vec<_> = console
            .records
            .iter()
            .filter(|r| r.message_type == "error")
            .collect();
        if !errors.is_empty() {
            for e in &errors {
                eprintln!("console error: {} ({:?})", e.text, e.location);
            }
            return Err(format!(
                "{} console error(s) during page load",
                errors.len()
            ));
        }

        let network = page.network_records(false, false);
        let failed: Vec<_> = network
            .records
            .iter()
            .filter(|r| {
                r.failure.is_some() || matches!(r.response_status, Some(status) if status >= 400)
            })
            .collect();
        if !failed.is_empty() {
            for f in &failed {
                eprintln!(
                    "failed request: {} {} -> status={:?} failure={:?}",
                    f.method, f.url, f.response_status, f.failure
                );
            }
            return Err(format!(
                "{} failed network request(s) during page load",
                failed.len()
            ));
        }

        page.close(Default::default()).map_err(|e| e.to_string())
    })();

    let _ = browser.close();
    result.expect("browser smoke test failed");
}

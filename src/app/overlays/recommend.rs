//! The resource recommendation panel: current requests and limits against what
//! two weeks of usage justifies.
//!
//! Scans run on demand, never on a timer — each one is a long Prometheus query
//! per workload — so the panel opens empty and waits for the button.

use leptos::prelude::*;
use roder_core::{AdviceSeverity, ResourceAdvice, ResourceScan, ScanTotals};

use crate::app::detail::metrics::{format_cpu, format_mem};
use crate::app::state::{RecommendData, RecommendError, RecommendOpen, RecommendScanning};
use crate::app::ui::{use_bool_overlay, use_dialog_focus};

#[component]
pub(crate) fn RecommendPanel() -> impl IntoView {
    let open = expect_context::<RecommendOpen>().0;
    let data = expect_context::<RecommendData>().0;
    let scanning = expect_context::<RecommendScanning>().0;
    let error = expect_context::<RecommendError>().0;
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let (visible, closing, do_close) = use_bool_overlay(open);
    let dialog_ref = NodeRef::<leptos::html::Div>::new();
    use_dialog_focus(dialog_ref);

    // A cluster-wide scan is mostly workloads that are already sized correctly,
    // so the default view is the subset worth acting on.
    let only_actionable = RwSignal::new(true);

    let scan = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            if scanning.get_untracked() {
                return;
            }
            scanning.set(true);
            error.set(None);
            let namespace = selected_ns.get_untracked();
            leptos::task::spawn_local(async move {
                match crate::app::fetch_recommendations(namespace).await {
                    Ok(scan) => data.set(Some(scan)),
                    Err(message) => error.set(Some(message)),
                }
                scanning.set(false);
            });
        }
    };

    let rows = Memo::new(move |_| {
        let Some(scan) = data.get() else {
            return Vec::new();
        };
        scan.rows
            .into_iter()
            .filter(|row| !only_actionable.get() || is_actionable(row))
            .collect::<Vec<_>>()
    });

    view! {
        <Show when=move || visible.get()>
            <div class="alerts-scrim" on:click=move |_| do_close()></div>
            <div class="alerts-panel recommend-panel" class:closing=move || closing.get()
                node_ref=dialog_ref role="dialog" aria-modal="true" tabindex="-1">
                <div class="alerts-header">
                    <div class="alerts-heading">
                        <span class="alerts-title">"Resource Recommendations"</span>
                        <span class="alerts-refreshed" class:error=move || error.get().is_some()>
                            {move || scan_status(
                                data.get().as_ref(),
                                scanning.get(),
                                error.get().as_deref(),
                                selected_ns.get().as_deref(),
                            )}
                        </span>
                    </div>
                    <button
                        class="alerts-silence-toggle"
                        class:active=move || only_actionable.get()
                        data-tip="Hide containers that are already sized correctly"
                        on:click=move |_| only_actionable.update(|value| *value = !*value)
                    >
                        "Actionable"
                    </button>
                    <button class="alerts-refresh" disabled=move || scanning.get() on:click=scan>
                        {move || if scanning.get() { "Scanning..." } else { "Scan" }}
                    </button>
                    <button class="alerts-close" on:click=move |_| do_close()>"✕"</button>
                </div>

                {move || data.get().map(|scan| view! { <ScanSummary totals=scan.totals() /> })}

                <div class="alerts-list recommend-list">
                    <Show when=move || data.get().is_none() && !scanning.get()>
                        <p class="alerts-empty">
                            "Scan to compare requests and limits against usage history."
                        </p>
                    </Show>
                    <Show when=move || data.get().is_some() && rows.get().is_empty()>
                        <p class="alerts-empty">
                            {move || if only_actionable.get() {
                                "Every container is sized about right."
                            } else {
                                "No containers to report."
                            }}
                        </p>
                    </Show>
                    <Show when=move || !rows.get().is_empty()>
                        <table class="recommend-table">
                            <thead>
                                <tr class="rt-group">
                                    <th rowspan="2">"Workload"</th>
                                    <th rowspan="2">"Container"</th>
                                    <th colspan="2" class="rt-cpu">"CPU"</th>
                                    <th colspan="2" class="rt-mem">"Memory"</th>
                                </tr>
                                <tr>
                                    <th class="rt-cpu">"Request"</th>
                                    <th>"Limit"</th>
                                    <th class="rt-mem">"Request"</th>
                                    <th>"Limit"</th>
                                </tr>
                            </thead>
                            <tbody>
                                <For
                                    each=move || rows.get()
                                    key=|row| (
                                        row.namespace.clone(),
                                        row.workload.clone(),
                                        row.container.clone(),
                                    )
                                    let:row
                                >
                                    <AdviceRow row />
                                </For>
                            </tbody>
                        </table>
                    </Show>
                    {move || failures(data.get().as_ref())}
                </div>
            </div>
        </Show>
    }
}

/// The rollup above the table: what applying everything would reclaim.
#[component]
fn ScanSummary(totals: ScanTotals) -> impl IntoView {
    let cpu = totals.cpu_delta();
    let memory = totals.memory_delta();

    view! {
        <div class="rsum">
            <Reclaim label="CPU" delta=cpu format=format_cpu />
            <Reclaim label="Memory" delta=memory format=format_mem />
            <span class="rsum-sep"></span>
            <Count severity=AdviceSeverity::Critical count=totals.critical />
            <Count severity=AdviceSeverity::Warning count=totals.warning />
            <Count severity=AdviceSeverity::Ok count=totals.ok />
            <Count severity=AdviceSeverity::Good count=totals.good />
            <Show when=move || { totals.unset > 0 }>
                <span class="rsum-chip" data-tip="Containers with no request declared">
                    <b>{totals.unset}</b>" unset"
                </span>
            </Show>
            <Show when=move || { totals.droppable_cpu_limits > 0 }>
                <span class="rsum-chip"
                    data-tip="CPU limits the scan advises removing — throttling is harder to diagnose than a noisy neighbour">
                    <b>{totals.droppable_cpu_limits}</b>" CPU limits to drop"
                </span>
            </Show>
        </div>
    }
}

/// A headline figure: cores or bytes freed (or needed) across the whole scan.
#[component]
fn Reclaim(label: &'static str, delta: f64, format: fn(f64) -> String) -> impl IntoView {
    let reclaims = delta >= 0.0;
    let tip = if reclaims {
        format!("{label} freed by applying every recommendation")
    } else {
        format!(
            "Additional {} needed — the cluster under-requests",
            label.to_lowercase()
        )
    };

    view! {
        <span class="rsum-stat" class:down=reclaims class:up=!reclaims data-tip=tip>
            <span class="rsum-label">{label}</span>
            <b>{format!("{}{}", if reclaims { "−" } else { "+" }, format(delta.abs()))}</b>
        </span>
    }
}

#[component]
fn Count(severity: AdviceSeverity, count: usize) -> impl IntoView {
    view! {
        <Show when=move || { count > 0 }>
            <span class="rsum-chip" data-tip=severity.label()>
                <span class=format!("sev-dot {}", severity.dot_class())></span>
                <b>{count}</b>
            </span>
        </Show>
    }
}

#[component]
fn AdviceRow(row: ResourceAdvice) -> impl IntoView {
    let severity = row.severity();
    let namespace = row.namespace.clone();
    let workload = row.workload.clone();
    let kind = row.workload_kind.label();
    let info = row.info.clone();
    let current = row.current;
    let recommended = row.recommended;

    view! {
        <tr class=format!("rt-row rt-{}", severity_slug(severity))>
            <td class="rt-workload">
                <span class="rt-ns">{namespace}</span>
                <span class="rt-name">{workload}</span>
                <span class="pill rt-kind">{kind}</span>
            </td>
            <td class="rt-container">{row.container.clone()}</td>
            <td class="rt-cpu">
                <Cell current=current.cpu_request recommended=recommended.cpu_request
                    format=format_cpu />
            </td>
            <td>
                <Cell current=current.cpu_limit recommended=recommended.cpu_limit
                    format=format_cpu />
            </td>
            <td class="rt-mem">
                <Cell current=current.memory_request recommended=recommended.memory_request
                    format=format_mem />
            </td>
            <td>
                <Cell current=current.memory_limit recommended=recommended.memory_limit
                    format=format_mem />
            </td>
        </tr>
        {info.map(|info| view! {
            <tr class="recommend-note"><td colspan="6">{info}</td></tr>
        })}
    }
}

/// One value: `current → recommended`, the percentage change, and a two-track
/// bar sizing both against the larger of them.
#[component]
fn Cell(
    current: Option<f64>,
    recommended: Option<f64>,
    format: fn(f64) -> String,
) -> impl IntoView {
    // Nothing set and nothing advised — mostly the CPU limit column, which KRR
    // wants empty. "unset → unset" over two blank tracks was pure noise.
    if current.is_none() && recommended.is_none() {
        return view! { <span class="rc-empty">"—"</span> }.into_any();
    }

    let current_label = current.map_or_else(|| "unset".to_string(), format);
    let recommended_label = recommended.map(format);
    let change = percent_change(current, recommended);
    let direction = direction(current, recommended);
    let (current_width, recommended_width) = bar_widths(current, recommended);

    view! {
        <span class="rc">
            <span class="rc-values">
                <span class="rc-current" class:unset=current.is_none()>{current_label}</span>
                <span class="rc-arrow">{if recommended_label.is_some() { "→" } else { "" }}</span>
                <span class=format!("rc-target {direction}")>
                    {recommended_label.unwrap_or_else(|| "unset".to_string())}
                </span>
            </span>
            <span class="rc-foot">
                <span class="rc-bar" aria-hidden="true">
                    <span class="rc-track">
                        <span class="rc-fill cur" style=format!("width:{current_width:.0}%")></span>
                    </span>
                    <span class="rc-track">
                        <span class=format!("rc-fill rec {direction}")
                            style=format!("width:{recommended_width:.0}%")></span>
                    </span>
                </span>
                <span class=format!("rc-change {direction}")>{change}</span>
            </span>
        </span>
    }
    .into_any()
}

/// `"up"` when the recommendation asks for more than is set, `"down"` when it
/// frees capacity, `"flat"` when there is nothing to compare.
fn direction(current: Option<f64>, recommended: Option<f64>) -> &'static str {
    match (current, recommended) {
        (Some(current), Some(recommended)) if recommended > current => "up",
        (Some(current), Some(recommended)) if recommended < current => "down",
        // Nothing set but something recommended is an increase from zero.
        (None, Some(_)) => "up",
        // Something set but nothing recommended — for CPU limits, a removal.
        (Some(_), None) => "down",
        _ => "flat",
    }
}

/// Both bars are sized against the larger value, so the pair reads as a ratio.
fn bar_widths(current: Option<f64>, recommended: Option<f64>) -> (f64, f64) {
    let current = current.unwrap_or(0.0).max(0.0);
    let recommended = recommended.unwrap_or(0.0).max(0.0);
    let max = current.max(recommended);
    if max <= 0.0 || !max.is_finite() {
        return (0.0, 0.0);
    }
    (current / max * 100.0, recommended / max * 100.0)
}

/// Percentage change from current to recommended.
///
/// Growth from an unset or zero request reads as "new", not `inf%`.
fn percent_change(current: Option<f64>, recommended: Option<f64>) -> String {
    match (current, recommended) {
        (Some(current), Some(recommended)) if current > 0.0 => {
            let change = (recommended - current) / current * 100.0;
            if change.abs() < 1.0 {
                "~same".to_string()
            } else {
                format!("{change:+.0}%")
            }
        }
        (_, Some(_)) => "new".to_string(),
        (Some(_), None) => "drop".to_string(),
        (None, None) => String::new(),
    }
}

fn severity_slug(severity: AdviceSeverity) -> &'static str {
    match severity {
        AdviceSeverity::Critical => "critical",
        AdviceSeverity::Warning => "warning",
        AdviceSeverity::Ok => "ok",
        AdviceSeverity::Good => "good",
        AdviceSeverity::Unknown => "unknown",
    }
}

/// Whether a row is worth showing when the panel is filtered.
///
/// Rows with no recommendation are kept: "not enough data" and "HPA detected"
/// are answers the user asked for, and silently dropping them makes the scan
/// look like it skipped workloads.
fn is_actionable(row: &ResourceAdvice) -> bool {
    row.info.is_some() || row.missing_requests() || row.severity() > AdviceSeverity::Good
}

fn scan_status(
    scan: Option<&ResourceScan>,
    scanning: bool,
    error: Option<&str>,
    namespace: Option<&str>,
) -> String {
    if scanning {
        return match namespace {
            Some(namespace) => format!("Scanning {namespace}..."),
            None => "Scanning all namespaces...".to_string(),
        };
    }
    if let Some(error) = error {
        return format!("Scan failed: {error}");
    }
    let Some(scan) = scan else {
        return match namespace {
            Some(namespace) => format!("Ready to scan {namespace}"),
            None => "Ready to scan all namespaces".to_string(),
        };
    };
    format!(
        "{} workloads · p{:.0} CPU over {:.0}h",
        scan.scanned_workloads, scan.cpu_percentile, scan.history_hours
    )
}

/// Per-workload query failures, shown rather than swallowed — a scan that
/// silently covered less than it claimed would be worse than a noisy one.
fn failures(scan: Option<&ResourceScan>) -> Option<AnyView> {
    let scan = scan?;
    if scan.failures.is_empty() {
        return None;
    }
    let lines: Vec<String> = scan
        .failures
        .iter()
        .map(|failure| {
            format!(
                "{}/{}: {}",
                failure.namespace, failure.workload, failure.error
            )
        })
        .collect();
    let count = lines.len();
    Some(
        view! {
            <div class="recommend-failures">
                <span class="recommend-failures-title">
                    {format!("{count} workload(s) could not be scanned")}
                </span>
                <For each=move || lines.clone() key=|line| line.clone() let:line>
                    <span class="recommend-failure">{line}</span>
                </For>
            </div>
        }
        .into_any(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use roder_core::{ContainerResources, WorkloadKind};

    fn row(severity: AdviceSeverity, current: ContainerResources) -> ResourceAdvice {
        ResourceAdvice {
            namespace: "prod".to_string(),
            workload: "web".to_string(),
            workload_kind: WorkloadKind::Deployment,
            container: "app".to_string(),
            current,
            recommended: ContainerResources::default(),
            cpu_severity: severity,
            memory_severity: severity,
            info: None,
        }
    }

    fn sized() -> ContainerResources {
        ContainerResources {
            cpu_request: Some(0.1),
            memory_request: Some(1024.0),
            ..ContainerResources::default()
        }
    }

    #[test]
    fn correctly_sized_containers_are_filtered_out() {
        assert!(!is_actionable(&row(AdviceSeverity::Good, sized())));
    }

    #[test]
    fn drift_missing_requests_and_notes_all_stay_visible() {
        assert!(is_actionable(&row(AdviceSeverity::Ok, sized())));
        assert!(is_actionable(&row(
            AdviceSeverity::Good,
            ContainerResources::default()
        )));

        let mut skipped = row(AdviceSeverity::Unknown, sized());
        skipped.info = Some("HPA detected".to_string());
        assert!(is_actionable(&skipped));
    }

    #[test]
    fn percentage_change_reports_the_delta_in_both_directions() {
        assert_eq!(percent_change(Some(0.1), Some(0.9)), "+800%");
        assert_eq!(percent_change(Some(1.0), Some(0.25)), "-75%");
        assert_eq!(percent_change(Some(1.0), Some(1.002)), "~same");
    }

    #[test]
    fn growth_from_nothing_is_not_an_infinite_percentage() {
        // No current request, so there is no base to divide by.
        assert_eq!(percent_change(None, Some(0.5)), "new");
        assert_eq!(percent_change(Some(0.0), Some(0.5)), "new");
        // A CPU limit the scan wants removed.
        assert_eq!(percent_change(Some(2.0), None), "drop");
        assert_eq!(percent_change(None, None), "");
    }

    #[test]
    fn bars_size_both_values_against_the_larger() {
        assert_eq!(bar_widths(Some(1.0), Some(0.25)), (100.0, 25.0));
        assert_eq!(bar_widths(Some(0.5), Some(1.0)), (50.0, 100.0));
        // Unset renders as an empty track rather than a divide-by-zero.
        assert_eq!(bar_widths(None, Some(2.0)), (0.0, 100.0));
        assert_eq!(bar_widths(None, None), (0.0, 0.0));
    }

    #[test]
    fn direction_marks_removals_as_freeing_capacity() {
        assert_eq!(direction(Some(1.0), Some(2.0)), "up");
        assert_eq!(direction(Some(2.0), Some(1.0)), "down");
        assert_eq!(direction(None, Some(1.0)), "up");
        // A CPU limit being dropped.
        assert_eq!(direction(Some(1.0), None), "down");
        assert_eq!(direction(None, None), "flat");
    }

    #[test]
    fn status_names_the_scope_before_a_scan_has_run() {
        assert_eq!(
            scan_status(None, false, None, Some("prod")),
            "Ready to scan prod"
        );
        assert_eq!(
            scan_status(None, true, None, None),
            "Scanning all namespaces..."
        );
    }

    #[test]
    fn status_reports_the_settings_a_scan_ran_under() {
        let scan = ResourceScan {
            rows: Vec::new(),
            history_hours: 336.0,
            cpu_percentile: 95.0,
            scanned_workloads: 42,
            failures: Vec::new(),
        };
        assert_eq!(
            scan_status(Some(&scan), false, None, None),
            "42 workloads · p95 CPU over 336h"
        );
    }

    #[test]
    fn a_failed_scan_reports_why() {
        assert_eq!(
            scan_status(None, false, Some("503 Service Unavailable"), None),
            "Scan failed: 503 Service Unavailable"
        );
    }
}

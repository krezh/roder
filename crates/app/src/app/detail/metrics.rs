//! Metrics chart component for displaying CPU and memory usage over time.

use leptos::prelude::*;
use serde::Deserialize;

use crate::data;

/// A single metrics data point from the API.
#[derive(Clone, Deserialize)]
pub struct MetricsPoint {
    pub timestamp: u64,
    pub cpu: f64,
    pub mem: f64,
}

/// Chart component displaying CPU and memory usage over time.
#[component]
pub(crate) fn MetricsChart(namespace: String, name: String) -> impl IntoView {
    let metrics = LocalResource::new(move || {
        let ns = namespace.clone();
        let n = name.clone();
        async move {
            data::fetch_json::<Vec<MetricsPoint>>(&format!(
                "/api/metrics?namespace={}&name={}",
                ns, n
            ))
            .await
            .ok()
        }
    });

    view! {
        <div class="metrics-chart">
            <Suspense fallback=|| view! { <div class="pad muted">"Loading metrics…"</div> }>
                {move || metrics.get().flatten().map(|points| {
                    if points.is_empty() {
                        return view! { <div class="pad muted">"No metrics data yet. Waiting for metrics-server…"</div> }.into_any();
                    }
                    view! { <ChartInner points=points /> }.into_any()
                })}
            </Suspense>
        </div>
    }
}

#[component]
fn ChartInner(points: Vec<MetricsPoint>) -> impl IntoView {
    // Calculate chart dimensions
    let width = 500.0;
    let height = 200.0;
    let padding = 40.0;
    let chart_width = width - padding * 2.0;
    let chart_height = height - padding * 2.0;

    // Find min/max values for scaling
    let min_time = points.first().map(|p| p.timestamp).unwrap_or(0);
    let max_time = points.last().map(|p| p.timestamp).unwrap_or(0);
    let time_range = (max_time - min_time).max(1) as f64;

    let max_cpu = points.iter().map(|p| p.cpu).fold(0.0f64, |a, b| a.max(b));
    let max_mem = points.iter().map(|p| p.mem).fold(0.0f64, |a, b| a.max(b));

    // Scale functions
    let scale_x = move |t: u64| -> f64 {
        padding + ((t - min_time) as f64 / time_range) * chart_width
    };
    let scale_cpu = move |v: f64| -> f64 {
        height - padding - (v / max_cpu.max(0.001)) * chart_height
    };
    let scale_mem = move |v: f64| -> f64 {
        height - padding - (v / max_mem.max(1.0)) * chart_height
    };

    // Build SVG paths
    let cpu_path = points.iter().enumerate().map(|(i, p)| {
        let x = scale_x(p.timestamp);
        let y = scale_cpu(p.cpu);
        if i == 0 { format!("M {} {}", x, y) } else { format!("L {} {}", x, y) }
    }).collect::<Vec<_>>().join(" ");

    let mem_path = points.iter().enumerate().map(|(i, p)| {
        let x = scale_x(p.timestamp);
        let y = scale_mem(p.mem);
        if i == 0 { format!("M {} {}", x, y) } else { format!("L {} {}", x, y) }
    }).collect::<Vec<_>>().join(" ");

    // Format time labels
    let format_time = |t: u64| -> String {
        let secs = t - min_time;
        if secs < 60 { format!("{}s", secs) }
        else if secs < 3600 { format!("{}m", secs / 60) }
        else { format!("{}h", secs / 3600) }
    };

    let time_labels = [0.0, 0.25, 0.5, 0.75, 1.0].map(|pct| {
        let t = min_time + (time_range * pct) as u64;
        (scale_x(t), format_time(t))
    });

    // Format values for Y-axis
    let format_cpu = |v: f64| -> String {
        if v >= 1.0 { format!("{:.1}", v) } else { format!("{:.0}m", v * 1000.0) }
    };
    let format_mem = |v: f64| -> String {
        if v >= 1024.0 * 1024.0 * 1024.0 { format!("{:.1}Gi", v / (1024.0 * 1024.0 * 1024.0)) }
        else if v >= 1024.0 * 1024.0 { format!("{:.0}Mi", v / (1024.0 * 1024.0)) }
        else { format!("{:.0}Ki", v / 1024.0) }
    };

    let cpu_labels = [0.0, 0.5, 1.0].map(|pct| {
        let v = max_cpu * pct;
        (scale_cpu(v), format_cpu(v))
    });

    let mem_labels = [0.0, 0.5, 1.0].map(|pct| {
        let v = max_mem * pct;
        (scale_mem(v), format_mem(v))
    });

    view! {
        <div class="chart-container">
            <svg viewBox=format!("0 0 {} {}", width, height) class="chart-svg">
                // Grid lines
                <line x1=padding y1=padding x2=padding y2=height - padding stroke="var(--border)" stroke-width="1" />
                <line x1=padding y1=height - padding x2=width - padding y2=height - padding stroke="var(--border)" stroke-width="1" />

                // CPU line (blue)
                <path d=cpu_path fill="none" stroke="#58a6ff" stroke-width="2" />

                // Memory line (green)
                <path d=mem_path fill="none" stroke="#3fb950" stroke-width="2" />

                // Time labels (X-axis)
                {time_labels.iter().map(|(x, label)| view! {
                    <text x=*x y=height - 10.0 text-anchor="middle" class="chart-label">{label.clone()}</text>
                }).collect_view()}

                // CPU labels (Y-axis, left)
                {cpu_labels.iter().map(|(y, label)| view! {
                    <text x=5.0 y=*y text-anchor="start" class="chart-label chart-cpu">{label.clone()}</text>
                }).collect_view()}

                // Memory labels (Y-axis, right)
                {mem_labels.iter().map(|(y, label)| view! {
                    <text x=width - 5.0 y=*y text-anchor="end" class="chart-label chart-mem">{label.clone()}</text>
                }).collect_view()}
            </svg>
            <div class="chart-legend">
                <span class="legend-item"><span class="legend-dot" style="background:#58a6ff"></span>"CPU"</span>
                <span class="legend-item"><span class="legend-dot" style="background:#3fb950"></span>"Memory"</span>
            </div>
        </div>
    }
}

//! Metrics chart component for displaying CPU and memory usage over time.

use leptos::prelude::*;
use roder_core::MetricsPoint;

use crate::data;

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
    let cpu_values = points.iter().map(|p| p.cpu).collect();
    let mem_values = points.iter().map(|p| p.mem).collect();

    view! {
        <div class="metrics-graphs">
            <MetricGraph
                points=points.clone()
                values=cpu_values
                title="CPU"
                color="#58a6ff"
                minimum_max=0.001
                format_value=format_cpu
            />
            <MetricGraph
                points=points
                values=mem_values
                title="Memory"
                color="#3fb950"
                minimum_max=1.0
                format_value=format_mem
            />
        </div>
    }
}

#[component]
fn MetricGraph(
    points: Vec<MetricsPoint>,
    values: Vec<f64>,
    title: &'static str,
    color: &'static str,
    minimum_max: f64,
    format_value: fn(f64) -> String,
) -> impl IntoView {
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

    let max_value = values.iter().copied().fold(0.0f64, f64::max);

    // Scale functions
    let scale_x =
        move |t: u64| -> f64 { padding + ((t - min_time) as f64 / time_range) * chart_width };
    let scale_value =
        move |v: f64| -> f64 { height - padding - (v / max_value.max(minimum_max)) * chart_height };

    // Build SVG paths
    let path = points
        .iter()
        .zip(values.iter())
        .enumerate()
        .map(|(i, (p, value))| {
            let x = scale_x(p.timestamp);
            let y = scale_value(*value);
            if i == 0 {
                format!("M {} {}", x, y)
            } else {
                format!("L {} {}", x, y)
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    // Format time labels
    let format_time = |t: u64| -> String {
        let secs = t - min_time;
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else {
            format!("{}h", secs / 3600)
        }
    };

    let time_labels = [0.0, 0.25, 0.5, 0.75, 1.0].map(|pct| {
        let t = min_time + (time_range * pct) as u64;
        (scale_x(t), format_time(t))
    });

    let value_labels = [0.0, 0.5, 1.0].map(|pct| {
        let value = max_value * pct;
        (scale_value(value), format_value(value))
    });

    view! {
        <div class="chart-container">
            <div class="chart-title" style=format!("color:{color}")>{title}</div>
            <svg viewBox=format!("0 0 {} {}", width, height) class="chart-svg">
                // Grid lines
                <line x1=padding y1=padding x2=padding y2=height - padding stroke="var(--border)" stroke-width="1" />
                <line x1=padding y1=height - padding x2=width - padding y2=height - padding stroke="var(--border)" stroke-width="1" />

                <path d=path fill="none" stroke=color stroke-width="2" />

                // Time labels (X-axis)
                {time_labels.iter().map(|(x, label)| view! {
                    <text x=*x y=height - 10.0 text-anchor="middle" class="chart-label">{label.clone()}</text>
                }).collect_view()}

                {value_labels.iter().map(|(y, label)| view! {
                    <text x=5.0 y=*y text-anchor="start" class="chart-label">{label.clone()}</text>
                }).collect_view()}
            </svg>
        </div>
    }
}

fn format_cpu(v: f64) -> String {
    if v >= 1.0 {
        format!("{v:.1}")
    } else {
        format!("{:.0}m", v * 1000.0)
    }
}

fn format_mem(v: f64) -> String {
    if v >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1}Gi", v / (1024.0 * 1024.0 * 1024.0))
    } else if v >= 1024.0 * 1024.0 {
        format!("{:.0}Mi", v / (1024.0 * 1024.0))
    } else {
        format!("{:.0}Ki", v / 1024.0)
    }
}

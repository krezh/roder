//! Metrics chart component for displaying CPU and memory usage over time.

use leptos::prelude::*;
use roder_core::MetricsPoint;

use crate::data;

/// Chart component displaying CPU and memory usage over time.
#[component]
pub(crate) fn MetricsChart(namespace: String, name: String) -> impl IntoView {
    let refresh = RwSignal::new(0u64);
    let reconnect = RwSignal::new(0u64);
    let watch_namespace = namespace.clone();
    let watch_name = name.clone();
    Effect::new(move |_previous: Option<Option<data::SseHandle>>| {
        reconnect.track();
        let watched_name = watch_name.clone();
        let url = data::watch_url("/v1/Pod", Some(&watch_namespace), None);
        data::subscribe_with_error(
            &url,
            move |event| {
                if let roder_core::WatchEvent::Applied { row } = event {
                    if row.name == watched_name {
                        refresh.update(|tick| *tick = tick.wrapping_add(1));
                    }
                }
            },
            move || {
                set_timeout(
                    move || reconnect.update(|attempt| *attempt = attempt.wrapping_add(1)),
                    data::reconnect_delay(),
                )
            },
        )
    });

    let metrics = LocalResource::new(move || {
        refresh.track();
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
    let visible_metrics = RwSignal::new(None::<Vec<MetricsPoint>>);
    Effect::new(move |_| {
        let Some(points) = metrics.get().flatten() else {
            return;
        };
        if visible_metrics.get_untracked().as_ref() != Some(&points) {
            visible_metrics.set(Some(points));
        }
    });

    view! {
        <div class="metrics-chart">
            <Show
                when=move || visible_metrics.with(|points| points.as_ref().is_some_and(|p| !p.is_empty()))
                fallback=move || visible_metrics.with(|points| match points {
                    None => view! { <div class="pad muted">"Loading metrics…"</div> }.into_any(),
                    Some(_) => view! { <div class="pad muted">"No metrics data yet. Waiting for metrics-server…"</div> }.into_any(),
                })
            >
                <ChartInner points=visible_metrics />
            </Show>
        </div>
    }
}

#[component]
fn ChartInner(points: RwSignal<Option<Vec<MetricsPoint>>>) -> impl IntoView {
    view! {
        <div class="metrics-graphs">
            <MetricGraph
                points
                canvas_id="metrics-cpu-canvas"
                title="CPU"
                color="#58a6ff"
                minimum_max=0.001
                value=|point| point.cpu
                format_value=format_cpu
            />
            <MetricGraph
                points=points
                canvas_id="metrics-memory-canvas"
                title="Memory"
                color="#3fb950"
                minimum_max=1.0
                value=|point| point.mem
                format_value=format_mem
            />
        </div>
    }
}

#[component]
fn MetricGraph(
    points: RwSignal<Option<Vec<MetricsPoint>>>,
    canvas_id: &'static str,
    title: &'static str,
    color: &'static str,
    minimum_max: f64,
    value: fn(&MetricsPoint) -> f64,
    format_value: fn(f64) -> String,
) -> impl IntoView {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (points, minimum_max, value, format_value);

    #[cfg(target_arch = "wasm32")]
    {
        use send_wrapper::SendWrapper;

        let animator = StoredValue::new(SendWrapper::new(None::<GraphAnimator>));
        Effect::new(move |_| {
            let Some(history) = points.get() else {
                return;
            };
            animator.update_value(|animator| {
                if let Some(animator) = animator.as_mut() {
                    animator.update(&history);
                } else {
                    **animator = GraphAnimator::start(
                        canvas_id,
                        &history,
                        title,
                        value,
                        format_value,
                        minimum_max,
                        color,
                    );
                }
            });
        });
        on_cleanup(move || {
            animator.update_value(|animator| {
                if let Some(animator) = (**animator).take() {
                    animator.stop();
                }
            });
        });
    }

    view! {
        <div class="chart-container">
            <div class="chart-title" style=format!("color:{color}")>{title}</div>
            <canvas id=canvas_id class="chart-canvas"></canvas>
        </div>
    }
}

#[cfg(target_arch = "wasm32")]
// metrics-server samples every 15 seconds. Thirteen visible points plus the
// off-screen incoming point produce the same three-minute window as the
// reference graph's 90 points at two-second intervals.
const GRAPH_POINTS: usize = 14;
#[cfg(target_arch = "wasm32")]
const SAMPLE_INTERVAL_MS: f64 = 15_000.0;

#[cfg(target_arch = "wasm32")]
type FrameCallback =
    std::rc::Rc<std::cell::RefCell<Option<wasm_bindgen::closure::Closure<dyn FnMut(f64)>>>>;

#[cfg(target_arch = "wasm32")]
struct GraphAnimator {
    state: std::rc::Rc<std::cell::RefCell<GraphState>>,
    frame: FrameCallback,
    frame_id: std::rc::Rc<std::cell::Cell<i32>>,
    mousemove: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MouseEvent)>,
    mouseleave: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MouseEvent)>,
}

#[cfg(target_arch = "wasm32")]
struct GraphState {
    canvas: web_sys::HtmlCanvasElement,
    context: web_sys::CanvasRenderingContext2d,
    tooltip: web_sys::Element,
    values: [f64; GRAPH_POINTS],
    last_timestamp: u64,
    last_push_ms: f64,
    minimum_max: f64,
    title: &'static str,
    color: &'static str,
    format_value: fn(f64) -> String,
    value: fn(&MetricsPoint) -> f64,
}

#[cfg(target_arch = "wasm32")]
impl GraphAnimator {
    fn start(
        canvas_id: &str,
        history: &[MetricsPoint],
        title: &'static str,
        value: fn(&MetricsPoint) -> f64,
        format_value: fn(f64) -> String,
        minimum_max: f64,
        color: &'static str,
    ) -> Option<Self> {
        use wasm_bindgen::JsCast;

        let window = web_sys::window()?;
        let document = window.document()?;
        let canvas = document
            .get_element_by_id(canvas_id)?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .ok()?;
        let context = canvas
            .get_context("2d")
            .ok()??
            .dyn_into::<web_sys::CanvasRenderingContext2d>()
            .ok()?;
        let tooltip = document.create_element("div").ok()?;
        tooltip.set_class_name("tooltip metrics-graph-tooltip");
        document.body()?.append_child(&tooltip).ok()?;
        let mut values = [0.0; GRAPH_POINTS];
        let history = &history[history.len().saturating_sub(GRAPH_POINTS)..];
        let offset = GRAPH_POINTS - history.len();
        for (index, point) in history.iter().enumerate() {
            values[offset + index] = value(point);
        }
        let state = std::rc::Rc::new(std::cell::RefCell::new(GraphState {
            canvas,
            context,
            tooltip,
            values,
            last_timestamp: history.last().map_or(0, |point| point.timestamp),
            last_push_ms: js_sys::Date::now() - SAMPLE_INTERVAL_MS,
            minimum_max,
            title,
            color,
            format_value,
            value,
        }));
        let frame: FrameCallback = std::rc::Rc::new(std::cell::RefCell::new(None));
        let frame_id = std::rc::Rc::new(std::cell::Cell::new(0));
        let state_for_frame = state.clone();
        let frame_for_frame = frame.clone();
        let id_for_frame = frame_id.clone();
        *frame.borrow_mut() = Some(wasm_bindgen::closure::Closure::new(move |_now| {
            state_for_frame.borrow_mut().draw(js_sys::Date::now());
            if let (Some(window), Some(callback)) =
                (web_sys::window(), frame_for_frame.borrow().as_ref())
            {
                if let Ok(id) = window.request_animation_frame(callback.as_ref().unchecked_ref()) {
                    id_for_frame.set(id);
                }
            }
        }));
        let state_for_move = state.clone();
        let mousemove = wasm_bindgen::closure::Closure::new(move |event: web_sys::MouseEvent| {
            state_for_move.borrow().show_tooltip(&event);
        });
        state
            .borrow()
            .canvas
            .add_event_listener_with_callback("mousemove", mousemove.as_ref().unchecked_ref())
            .ok()?;
        let state_for_leave = state.clone();
        let mouseleave = wasm_bindgen::closure::Closure::new(move |_event: web_sys::MouseEvent| {
            state_for_leave.borrow().hide_tooltip();
        });
        state
            .borrow()
            .canvas
            .add_event_listener_with_callback("mouseleave", mouseleave.as_ref().unchecked_ref())
            .ok()?;
        if let Some(callback) = frame.borrow().as_ref() {
            let id = window
                .request_animation_frame(callback.as_ref().unchecked_ref())
                .ok()?;
            frame_id.set(id);
        }
        Some(Self {
            state,
            frame,
            frame_id,
            mousemove,
            mouseleave,
        })
    }

    fn update(&mut self, history: &[MetricsPoint]) {
        let mut state = self.state.borrow_mut();
        for point in history {
            if point.timestamp <= state.last_timestamp {
                continue;
            }
            let value = (state.value)(point);
            state.values.rotate_left(1);
            state.values[GRAPH_POINTS - 1] = value;
            state.last_timestamp = point.timestamp;
            state.last_push_ms = js_sys::Date::now();
        }
    }

    fn stop(self) {
        use wasm_bindgen::JsCast;

        if let Some(window) = web_sys::window() {
            let _ = window.cancel_animation_frame(self.frame_id.get());
        }
        let state = self.state.borrow();
        let _ = state.canvas.remove_event_listener_with_callback(
            "mousemove",
            self.mousemove.as_ref().unchecked_ref(),
        );
        let _ = state.canvas.remove_event_listener_with_callback(
            "mouseleave",
            self.mouseleave.as_ref().unchecked_ref(),
        );
        state.tooltip.remove();
        self.frame.borrow_mut().take();
    }
}

#[cfg(target_arch = "wasm32")]
impl GraphState {
    fn show_tooltip(&self, event: &web_sys::MouseEvent) {
        let rect = self.canvas.get_bounding_client_rect();
        let width = rect.width();
        let left = 42.0;
        let right = width - 8.0;
        let plot_width = (right - left).max(1.0);
        let slot_width = plot_width / (GRAPH_POINTS as f64 - 2.0);
        let phase =
            ((js_sys::Date::now() - self.last_push_ms) / SAMPLE_INTERVAL_MS).clamp(0.0, 1.0);
        let offset = phase * slot_width;
        let relative_x = event.client_x() as f64 - rect.left();
        let slot = ((relative_x - left + offset) / slot_width)
            .round()
            .clamp(0.0, (GRAPH_POINTS - 1) as f64) as usize;
        self.tooltip.set_text_content(Some(&format!(
            "{}: {}",
            self.title,
            (self.format_value)(self.values[slot])
        )));
        let _ = self.tooltip.set_attribute(
            "style",
            &format!(
                "display:block;left:{}px;top:{}px",
                event.client_x(),
                event.client_y()
            ),
        );

        let tooltip_rect = self.tooltip.get_bounding_client_rect();
        let viewport_width = web_sys::window()
            .and_then(|window| window.inner_width().ok())
            .and_then(|width| width.as_f64())
            .unwrap_or(width);
        let margin = 8.0;
        let tooltip_left = (event.client_x() as f64 - tooltip_rect.width() / 2.0).clamp(
            margin,
            (viewport_width - tooltip_rect.width() - margin).max(margin),
        );
        let tooltip_top = (event.client_y() as f64 - tooltip_rect.height() - 12.0).max(margin);
        let _ = self.tooltip.set_attribute(
            "style",
            &format!("display:block;left:{tooltip_left}px;top:{tooltip_top}px"),
        );
    }

    fn hide_tooltip(&self) {
        let _ = self.tooltip.set_attribute("style", "display:none");
    }

    fn draw(&mut self, now: f64) {
        let css_width = self.canvas.client_width().max(1) as f64;
        let css_height = self.canvas.client_height().max(1) as f64;
        let ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
        let pixel_width = (css_width * ratio) as u32;
        let pixel_height = (css_height * ratio) as u32;
        if self.canvas.width() != pixel_width || self.canvas.height() != pixel_height {
            self.canvas.set_width(pixel_width);
            self.canvas.set_height(pixel_height);
            let _ = self.context.scale(ratio, ratio);
        }

        let left = 42.0;
        let right = css_width - 8.0;
        let top = 8.0;
        let bottom = css_height - 22.0;
        let plot_width = (right - left).max(1.0);
        let plot_height = (bottom - top).max(1.0);
        let slot_width = plot_width / (GRAPH_POINTS as f64 - 2.0);
        let phase = ((now - self.last_push_ms) / SAMPLE_INTERVAL_MS).clamp(0.0, 1.0);
        let offset = phase * slot_width;
        let max_value = self.values.iter().copied().fold(self.minimum_max, f64::max);
        let y = |value: f64| bottom - (value / max_value) * plot_height;

        self.context.clear_rect(0.0, 0.0, css_width, css_height);
        self.context.set_stroke_style_str("rgba(127,127,127,.25)");
        self.context.set_line_width(1.0);
        self.context.begin_path();
        self.context.move_to(left, top);
        self.context.line_to(left, bottom);
        self.context.line_to(right, bottom);
        self.context.stroke();

        self.context.save();
        self.context.begin_path();
        self.context.rect(left, top, plot_width, plot_height);
        self.context.clip();
        self.context.set_stroke_style_str(self.color);
        self.context.set_line_width(2.0);
        self.context.begin_path();
        for index in 0..GRAPH_POINTS {
            let x = left + index as f64 * slot_width - offset;
            let point_y = y(self.values[index]);
            if index == 0 {
                self.context.move_to(x, point_y);
            } else {
                let previous_x = left + (index - 1) as f64 * slot_width - offset;
                let previous_y = y(self.values[index - 1]);
                let control_x = (previous_x + x) / 2.0;
                self.context
                    .bezier_curve_to(control_x, previous_y, control_x, point_y, x, point_y);
            }
        }
        self.context.stroke();
        self.context.restore();

        self.context.set_fill_style_str("#8b949e");
        self.context.set_font("9px sans-serif");
        let _ = self
            .context
            .fill_text(&(self.format_value)(max_value), 4.0, top + 4.0);
        let _ = self.context.fill_text(
            &(self.format_value)(max_value / 2.0),
            4.0,
            top + plot_height / 2.0 + 3.0,
        );
        let _ = self
            .context
            .fill_text(&(self.format_value)(0.0), 4.0, bottom + 3.0);
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

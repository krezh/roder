//! Full-screen mobile replacement for the desktop's `LogSidebar`: one log
//! pane at a time (a chip switcher when more than one source is open)
//! instead of side-by-side panes, since those don't fit a phone's width.

use leptos::prelude::*;

use crate::app::logs::LogsView;
use crate::app::state::LogPods;

#[component]
pub(crate) fn MobileLogsView() -> impl IntoView {
    let log_pods = expect_context::<LogPods>().0;
    let active = RwSignal::new(0usize);
    Effect::new(move |_| {
        let n = log_pods.with(|v| v.len());
        if n > 0 && active.get_untracked() >= n {
            active.set(n - 1);
        }
    });

    view! {
        <div class="mobile-logs" class:open=move || !log_pods.get().is_empty()>
            <div class="mobile-logs-head">
                <button class="mobile-logs-close-all" on:click=move |_| log_pods.set(Vec::new())>"‹ Close all"</button>
            </div>
            {move || {
                let pods = log_pods.get();
                (pods.len() > 1).then(|| view! {
                    <div class="mobile-pane-switcher">
                        {pods.iter().enumerate().map(|(i, t)| {
                            let title = if t.aggregate { format!("{} (all)", t.name) } else { t.name.clone() };
                            view! {
                                <span class="mobile-pane-chip" class:active=move || active.get() == i
                                    on:click=move |_| active.set(i)>{title}</span>
                            }
                        }).collect_view()}
                    </div>
                })
            }}
            <div class="mobile-logs-body">
                {move || {
                    let pods = log_pods.get();
                    if pods.is_empty() {
                        return None;
                    }
                    let i = active.get().min(pods.len() - 1);
                    pods.get(i).cloned().map(|t| {
                        let title = if t.aggregate { format!("{} (all)", t.name) } else { t.name.clone() };
                        let url = t.url();
                        view! { <LogsView url=url title=title target=t /> }
                    })
                }}
            </div>
        </div>
    }
}

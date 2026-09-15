//! The one place a resource cell is painted from its column's [`ColumnKind`].
//!
//! Both grids — the single-kind table (`components::kind_table`) and the
//! multi-kind search table (`views::search`) — render the same cells from the
//! same classification; they differ only in how they *resolve* a cell's value
//! (by index against one schema, or by header across several). That resolution
//! stays with each view; the painting lives here, so the two can't drift.
//!
//! [`ColumnKind::Name`] is deliberately not handled: `NameCell` needs the
//! row's selection state, so each view renders it directly.

use leptos::prelude::*;
use roder_core::{RowStatus, Trend};

use crate::app::columns::{bool_color, ColumnKind};
use crate::app::components::table::FlashTd;
use crate::app::util::color::{dot_class, pct_thresh_color};

/// Render one cell according to its column's kind.
///
/// `value` is read reactively; `FlashTd` humanizes timestamps and list values
/// itself, so callers pass the raw cell through unchanged.
#[component]
pub(crate) fn DataCell<V>(
    kind: ColumnKind,
    value: V,
    /// The owning row's status — read only by [`ColumnKind::Status`] cells,
    /// which paint their pill with it.
    #[prop(optional, into)]
    status: Option<Signal<RowStatus>>,
    #[prop(optional, into)] trend: Option<Signal<Trend>>,
    /// Drives the change highlight where the view tracks it per column.
    /// Omitted, `FlashTd` falls back to noticing the value change itself.
    #[prop(optional, into)]
    flash: Option<Signal<bool>>,
) -> impl IntoView
where
    V: Fn() -> String + Copy + Send + Sync + 'static,
{
    match kind {
        ColumnKind::Namespace => {
            view! { <FlashTd value=value class="cell-ns" flash=flash /> }.into_any()
        }
        ColumnKind::Age => {
            view! { <FlashTd value=value class="cell-age" no_flash=true /> }.into_any()
        }
        ColumnKind::Status => {
            let status = status.unwrap_or_else(|| Signal::derive(|| RowStatus::Unknown));
            view! {
                <FlashTd value=value flash=flash pill=true
                    color=Signal::derive(move || dot_class(status.get())) />
            }
            .into_any()
        }
        // The value's own colour carries the state change, so these don't also
        // flash — a mount/attach flip is already unmissable.
        ColumnKind::Bool => view! {
            <FlashTd value=value no_flash=true
                color=Signal::derive(move || bool_color(&value())) />
        }
        .into_any(),
        ColumnKind::Percent => view! {
            <FlashTd value=value no_flash=true trend=trend pct_bar=true
                color=Signal::derive(move || pct_thresh_color(&value())) />
        }
        .into_any(),
        // Metrics move on every scrape; flashing them would strobe the table.
        ColumnKind::Metric => {
            view! { <FlashTd value=value no_flash=true trend=trend /> }.into_any()
        }
        // `Name` is rendered by the view itself; falling through to plain text
        // keeps this total rather than panicking on a caller's mistake.
        ColumnKind::Name | ColumnKind::Plain => {
            view! { <FlashTd value=value flash=flash trend=trend /> }.into_any()
        }
    }
}

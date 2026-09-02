//! The desktop keymap: one declarative table that both dispatch and the `?`
//! overlay read from.
//!
//! Before this module the desktop bindings lived in two independent `window`
//! keydown listeners (`App` and `KindTable`) and were re-typed by hand a third
//! time as static markup in the help overlay, which had already drifted.
//! Everything now comes from [`BINDINGS`], so a binding that isn't listed
//! doesn't exist and a listed one can't be missing from the help.
//!
//! Desktop only — `mobile::shortcuts` keeps its own list and is not wired here.
//!
//! Dispatch is layered: [`Layer`] is computed once at App level from the overlay
//! signals, and each listener bails unless the active layer is its own. That is
//! what stops `l` (logs) from firing while the command palette has focus.

use std::collections::BTreeSet;
use std::time::Duration;

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;

use crate::app::controllers::detail::DetailTab;
use crate::app::hooks::{scroll_cursor_into_view, ResourceTable};
use crate::app::state::{
    open_logs, pinned_in_catalog_order, AccessReviewOpen, AlertsOpen, Catalog, CtxMenu,
    DetailTarget, DrainOpen, ExecOpen, ExecTarget, FileBrowserOpen, FilterFocus, LogPods,
    LogTarget, NsPaletteOpen, OnlyProblems, PaletteOpen, PinnedKinds, PodModalTarget,
    ShortcutsOpen, SortKey, TreeOpen,
};
use crate::app::table_logic::{bulk_targets, move_cursor};
use crate::app::ui::{
    ask_delete, delete_extra, show_toast, Confirm, DeleteRequest, Toast, ToastKind,
};
use crate::app::util::clipboard::copy_to_clipboard;
use crate::app::util::format::parse_key;
use crate::app::util::predicate::KindKind;

/// Which handler owns the keyboard right now.
///
/// Ordered by precedence, highest first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Layer {
    /// A modal overlay owns the keyboard: palette, namespace switcher, help,
    /// confirm/delete dialogs, exec, drain, tree, alerts. These bring their own
    /// handlers; the global and table maps stand down entirely.
    Overlay,
    /// The context menu is open — motion keys walk its items instead of rows.
    Menu,
    /// Nothing is over the table, so the full table map is live.
    Table,
}

/// Groups the help overlay renders as sections, in this order.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Group {
    Motion,
    Navigation,
    Selection,
    Actions,
    Danger,
    Menu,
}

impl Group {
    pub(crate) const ORDER: [Group; 6] = [
        Group::Motion,
        Group::Navigation,
        Group::Selection,
        Group::Actions,
        Group::Danger,
        Group::Menu,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Group::Motion => "Motion",
            Group::Navigation => "Navigation",
            Group::Selection => "Selection",
            Group::Actions => "Actions",
            Group::Danger => "Destructive",
            Group::Menu => "Actions menu",
        }
    }
}

/// One row of the help overlay, and the authoritative record that a binding
/// exists. `keys` holds the display forms of every alias, rendered as separate
/// `<kbd>` chips.
pub(crate) struct Binding {
    pub keys: &'static [&'static str],
    pub label: &'static str,
    pub group: Group,
    /// Shown in the help as a hint that the binding takes a `{count}` prefix.
    pub counted: bool,
}

const fn b(keys: &'static [&'static str], label: &'static str, group: Group) -> Binding {
    Binding {
        keys,
        label,
        group,
        counted: false,
    }
}

const fn bc(keys: &'static [&'static str], label: &'static str, group: Group) -> Binding {
    Binding {
        keys,
        label,
        group,
        counted: true,
    }
}

/// Every desktop binding. Keep this in the order the help reads.
///
/// A few choices that aren't self-evident:
///   * `Shift+N` is the namespace switcher rather than sort-by-name, because the
///     topbar renders "All namespaces ⇧N" as a visible affordance.
///   * Favourites sit on `Ctrl`+digit, leaving the plain digits free to be
///     motion counts (`5j`). They are the pinned *kinds* from the sidebar.
///   * `Shift+<letter>` is reserved for sorting, so per-column sort keys stay
///     available on kinds whose columns aren't known ahead of time.
pub(crate) const BINDINGS: &[Binding] = &[
    // Motion.
    bc(&["j", "↓"], "Next row", Group::Motion),
    bc(&["k", "↑"], "Previous row", Group::Motion),
    b(&["g"], "First row", Group::Motion),
    b(&["G"], "Last row", Group::Motion),
    b(&["Ctrl+f", "PgDn"], "Page down", Group::Motion),
    b(&["Ctrl+b", "PgUp"], "Page up", Group::Motion),
    // Navigation.
    b(&[":", "K"], "Kind palette", Group::Navigation),
    b(&["N"], "Namespace switcher", Group::Navigation),
    b(
        &["Ctrl+1", "…", "Ctrl+0"],
        "Jump to a pinned favorite",
        Group::Navigation,
    ),
    b(&["w"], "Workspace view", Group::Navigation),
    b(&["/"], "Filter current view", Group::Navigation),
    b(&["Ctrl+z", "E"], "Toggle problem filter", Group::Navigation),
    b(
        &["Shift+…"],
        "Sort by the column with that initial; again to reverse",
        Group::Navigation,
    ),
    b(&["?"], "Show this help", Group::Navigation),
    b(&["Esc"], "Back / close / clear marks", Group::Navigation),
    // Selection (marks).
    bc(&["Space"], "Mark row", Group::Selection),
    b(&["Ctrl+\\"], "Clear all marks", Group::Selection),
    b(&["Ctrl+a"], "Mark every shown row", Group::Selection),
    // Actions — operate on the marks, or on the cursor row when there are none.
    b(&["Enter", "d"], "Describe / open details", Group::Actions),
    b(&["y"], "YAML", Group::Actions),
    b(&["l"], "Logs", Group::Actions),
    b(&["s"], "Shell into pod", Group::Actions),
    b(&["r"], "Relationships", Group::Actions),
    b(&["a"], "Open the actions menu", Group::Actions),
    b(&["⌘C", "Ctrl+c"], "Copy name(s)", Group::Actions),
    // Destructive.
    b(&["Ctrl+d"], "Delete (asks first)", Group::Danger),
    b(
        &["Ctrl+k"],
        "Kill — force delete, no grace period",
        Group::Danger,
    ),
    // Context menu layer.
    b(&["j", "k", "↑", "↓"], "Move between items", Group::Menu),
    b(&["Enter"], "Run the highlighted item", Group::Menu),
    b(&["Esc"], "Close the menu", Group::Menu),
];

/// The pending `{count}` prefix — the digits typed before a motion.
///
/// There is no chord prefix: a single `g` reaches the top of the table, so
/// nothing here waits on a second key.
///
/// Cleared after [`PENDING_TIMEOUT_MS`] of inactivity, so an abandoned `5` can't
/// silently multiply a motion typed minutes later.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Pending {
    /// Digits typed so far; `0` means no count was given.
    pub count: u32,
}

/// How long a partial count survives before it is discarded.
pub(crate) const PENDING_TIMEOUT_MS: u64 = 900;

/// Counts above this are almost certainly a stuck key, and a huge repeat would
/// stall the UI while it walks the list.
const MAX_COUNT: u32 = 9999;

impl Pending {
    pub(crate) fn is_empty(self) -> bool {
        self.count == 0
    }

    /// The count to apply to a motion — at least 1.
    pub(crate) fn repeat(self) -> usize {
        self.count.max(1) as usize
    }

    /// Append a digit. Returns `false` if the digit should be treated as a plain
    /// key instead (a leading `0`, which is "line start" in vim, not a count).
    pub(crate) fn push_digit(&mut self, d: u32) -> bool {
        if d == 0 && self.count == 0 {
            return false;
        }
        self.count = (self.count.saturating_mul(10).saturating_add(d)).min(MAX_COUNT);
        true
    }

    /// The `showcmd` string rendered in the corner while a count is pending.
    pub(crate) fn display(self) -> String {
        if self.count == 0 {
            String::new()
        } else {
            self.count.to_string()
        }
    }
}

/// Context wrapper for the pending-key buffer, shared by the global and table
/// dispatchers so a `g` typed anywhere completes anywhere.
#[derive(Clone, Copy)]
pub(crate) struct PendingKeys(pub(crate) RwSignal<Pending>);

/// Context wrapper for the active layer.
#[derive(Clone, Copy)]
pub(crate) struct ActiveLayer(pub(crate) Signal<Layer>);

/// True when the event should be ignored outright because the user is typing
/// into a field. `Escape` is always let through so a filter box can be exited.
pub(crate) fn typing(e: &leptos::ev::KeyboardEvent) -> bool {
    e.key() != "Escape" && crate::data::is_text_input_focused()
}

/// What the dispatcher needs from whichever table currently owns the view.
///
/// `KindTable` registers this on mount when it is the primary table and clears
/// it on cleanup — the same pattern as [`crate::app::state::TableSelected`].
/// Routing the table bindings through a handle, instead of giving the table its
/// own `window` listener, is what guarantees a keypress is acted on once: the
/// desktop UI has exactly one `keydown` listener, and a workspace pane (which
/// passes `keyboard = false`) contributes no bindings at all.
#[derive(Clone, Copy)]
pub(crate) struct TableKeyHandle {
    pub table: ResourceTable,
    /// The filtered + sorted uids, in display order.
    pub shown: Memo<Vec<String>>,
    /// `group/version/Kind` of the rows, for building detail and log targets.
    pub kind_key: StoredValue<String>,
    /// Live column list, for `<` / `>` sort cycling.
    pub columns: RwSignal<Vec<String>>,
    /// uid -> node name, so a keyboard-opened context menu still offers
    /// "Go to node" for pods, exactly as a right-click does.
    pub node_for: Callback<String, Option<String>>,
}

#[derive(Clone, Copy)]
pub(crate) struct TableKeys(pub(crate) StoredValue<Option<TableKeyHandle>>);

/// The `SortKey` a column header binds to.
///
/// Must stay identical to the header in `kind_table`'s `header` closure — if
/// the two disagreed, `<` / `>` would sort by a different key than clicking the
/// same column does.
pub(crate) fn sort_key_for_column(columns: &[String], i: usize) -> SortKey {
    match columns.get(i).map(String::as_str) {
        Some("Namespace") => SortKey::Namespace,
        Some("Name") => SortKey::Name,
        Some("Age") => SortKey::Age,
        _ => SortKey::Cell(i),
    }
}

/// The column a `Shift+<letter>` sort selects.
///
/// Columns are dynamic — they come from each kind's `additionalPrinterColumns`
/// — so a fixed key-per-column table isn't possible. The letter picks the first
/// column with that initial instead, which gives the obvious keys on the built-in
/// kinds (`Shift+A` age, `Shift+S` status, `Shift+R` ready) and extends the same
/// habit to CRDs without any per-kind configuration.
///
/// Returns `None` when no column starts with the letter, so the keypress falls
/// through untouched rather than silently re-sorting by something arbitrary.
pub(crate) fn sort_column_for_letter(columns: &[String], letter: char) -> Option<SortKey> {
    let letter = letter.to_ascii_lowercase();
    let i = columns.iter().position(|c| {
        c.chars()
            .next()
            .is_some_and(|initial| initial.to_ascii_lowercase() == letter)
    })?;
    Some(sort_key_for_column(columns, i))
}

/// Install the single desktop `keydown` listener. Called once, by [`KeyLayer`].
fn use_key_dispatch() {
    let layer = expect_context::<ActiveLayer>().0;
    let pending = expect_context::<PendingKeys>().0;
    let keys = expect_context::<TableKeys>().0;

    let palette_open = expect_context::<PaletteOpen>().0;
    let ns_palette_open = expect_context::<NsPaletteOpen>().0;
    let shortcuts_open = expect_context::<ShortcutsOpen>().0;
    let alerts_open = expect_context::<AlertsOpen>().0;
    let access_review_open = expect_context::<AccessReviewOpen>().0;
    let exec_open = expect_context::<ExecOpen>().0;
    let file_browser_open = expect_context::<FileBrowserOpen>().0;
    let tree_open = expect_context::<TreeOpen>().0;
    let drain_open = expect_context::<DrainOpen>().0;
    let pod_modal = expect_context::<PodModalTarget>().0;
    let only_problems = expect_context::<OnlyProblems>().0;
    let filter_focus = expect_context::<FilterFocus>().0;
    let catalog = expect_context::<Catalog>().0;
    let pinned_kinds = expect_context::<PinnedKinds>().0;
    let selected_kind = expect_context::<RwSignal<Option<roder_core::ResourceKind>>>();
    let log_pods = expect_context::<LogPods>().0;
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    // Read once by the drawer as it mounts, so it must be set *before* `detail`.
    let requested_tab = expect_context::<RwSignal<Option<DetailTab>>>();
    let ctx_menu = expect_context::<RwSignal<Option<CtxMenu>>>();
    let confirm = expect_context::<RwSignal<Option<Confirm>>>();
    let delete_confirm = expect_context::<RwSignal<Option<DeleteRequest>>>();
    let toast = expect_context::<RwSignal<Option<Toast>>>();

    // The pending buffer self-clears, so a `g` or `5` abandoned mid-chord can't
    // silently capture the next real keypress minutes later.
    let timeout = StoredValue::new(None::<TimeoutHandle>);
    let arm = move |next: Pending| {
        timeout.update_value(|h| {
            if let Some(h) = h.take() {
                h.clear();
            }
        });
        pending.set(next);
        if !next.is_empty() {
            let handle = set_timeout_with_handle(
                move || pending.set(Pending::default()),
                Duration::from_millis(PENDING_TIMEOUT_MS),
            )
            .ok();
            timeout.set_value(handle);
        }
    };

    // Rows an action applies to: the selection when there is one, else the row
    // under the cursor — the same rule the context menu applies to a right-click.
    let action_uids = move |h: &TableKeyHandle| -> BTreeSet<String> {
        let selected = h.table.selected.get_untracked();
        if !selected.is_empty() {
            return selected;
        }
        h.table.cursor.get_untracked().into_iter().collect()
    };
    let action_targets = move |h: &TableKeyHandle| -> Vec<DetailTarget> {
        let uids = action_uids(h);
        let kind_key = h.kind_key.get_value();
        h.table
            .rows
            .with_untracked(|rows| bulk_targets(&kind_key, rows, &uids))
    };
    let move_by = move |h: &TableKeyHandle, delta: isize| {
        let shown = h.shown.get_untracked();
        let from = h.table.cursor.get_untracked();
        let Some(uid) = move_cursor(&shown, from.as_deref(), delta) else {
            return;
        };
        if let Some(i) = shown.iter().position(|u| u == &uid) {
            scroll_cursor_into_view(h.table, i);
        }
        h.table.cursor.set(Some(uid));
    };
    // A "page" is whatever currently fits on screen, so paging matches what the
    // user can actually see rather than a fixed row count.
    let page_rows = move |h: &TableKeyHandle| -> isize {
        let row_h = h.table.row_h.get_untracked().max(1.0);
        ((h.table.viewport_h.get_untracked() / row_h).floor() as isize).max(1)
    };

    let navigate = use_navigate();
    Effect::new(move |_| {
        let navigate = navigate.clone();
        let handle = window_event_listener(leptos::ev::keydown, move |e| {
            let key = e.key();
            let ctrl = e.ctrl_key() || e.meta_key();
            let current_layer = layer.get_untracked();

            // Escape unwinds exactly one level, top down. Owning it in one place
            // is what stops a single press from closing a dialog *and* clearing
            // the selection *and* closing the detail drawer all at once.
            if key == "Escape" {
                if !pending.get_untracked().is_empty() {
                    arm(Pending::default());
                    return;
                }
                if current_layer == Layer::Overlay {
                    // The palettes and the confirm/delete dialogs have no Escape
                    // handler of their own; they rely entirely on this one. Every
                    // signal that makes `Layer::Overlay` true must be cleared
                    // here, or that overlay would swallow Escape and get stuck.
                    palette_open.set(false);
                    ns_palette_open.set(false);
                    shortcuts_open.set(false);
                    alerts_open.set(false);
                    access_review_open.set(false);
                    exec_open.set(None);
                    file_browser_open.set(None);
                    tree_open.set(None);
                    drain_open.set(None);
                    pod_modal.set(None);
                    confirm.set(None);
                    delete_confirm.set(None);
                    return;
                }
                if current_layer == Layer::Menu {
                    ctx_menu.set(None);
                    return;
                }
                // Typing in the view filter: its own handler clears the text.
                // Anything more here would yank the detail drawer out from under
                // someone who only wanted to empty the filter box.
                if crate::data::is_text_input_focused() {
                    return;
                }
                if let Some(h) = keys.get_value() {
                    if !h.table.selected.with_untracked(BTreeSet::is_empty) {
                        h.table.selected.set(BTreeSet::new());
                        return;
                    }
                }
                // The log sidebar and detail drawer are panels rather than
                // modals, so they don't raise the layer — but Escape still has
                // to be able to dismiss them, innermost first.
                if !log_pods.with_untracked(Vec::is_empty) {
                    log_pods.set(Vec::new());
                    return;
                }
                detail.set(None);
                return;
            }

            // Everything past this point is a real binding, so a focused text
            // field swallows it and the non-table layers stand down.
            if typing(&e) {
                return;
            }
            if current_layer != Layer::Table {
                return;
            }

            // ---- count prefix ----------------------------------------------
            if !ctrl && key.len() == 1 {
                if let Some(d) = key.chars().next().and_then(|c| c.to_digit(10)) {
                    let mut next = pending.get_untracked();
                    if next.push_digit(d) {
                        arm(next);
                        e.prevent_default();
                        return;
                    }
                }
            }

            let count = pending.get_untracked();
            let repeat = count.repeat() as isize;
            // Any non-digit key consumes the pending buffer, whether or not it
            // turns out to be bound.
            if !count.is_empty() {
                arm(Pending::default());
            }

            // ---- bindings that work with or without a live table -----------
            // ---- Ctrl+1..Ctrl+0: jump to a pinned favourite --------------
            //
            // On Ctrl rather than the bare digits, which stay free for motion
            // counts. Numbering follows the sidebar's Favorites section, which
            // is catalog order.
            if ctrl && key.len() == 1 {
                if let Some(d) = key.chars().next().and_then(|c| c.to_digit(10)) {
                    let slot = if d == 0 { 9 } else { d as usize - 1 };
                    let pins = pinned_in_catalog_order(
                        &catalog.get_untracked(),
                        &pinned_kinds.get_untracked(),
                    );
                    if let Some(kind) = pins.into_iter().nth(slot) {
                        selected_kind.set(Some(kind));
                        e.prevent_default();
                    }
                    return;
                }
            }

            match key.as_str() {
                ":" | "K" => {
                    palette_open.set(true);
                    e.prevent_default();
                    return;
                }
                "N" => {
                    ns_palette_open.set(true);
                    e.prevent_default();
                    return;
                }
                "w" => {
                    navigate("/workspace", Default::default());
                    e.prevent_default();
                    return;
                }
                // Ctrl+Z is the faults-only filter; `E` predates this keymap and
                // stays as an alias.
                "z" if ctrl => {
                    only_problems.update(|v| *v = !*v);
                    e.prevent_default();
                    return;
                }
                "E" => {
                    only_problems.update(|v| *v = !*v);
                    e.prevent_default();
                    return;
                }
                "/" => {
                    filter_focus.update(|n| *n += 1);
                    e.prevent_default();
                    return;
                }
                "?" => {
                    shortcuts_open.update(|v| *v = !*v);
                    e.prevent_default();
                    return;
                }
                _ => {}
            }

            // ---- table bindings --------------------------------------------
            let Some(h) = keys.get_value() else { return };
            let kind_key = h.kind_key.get_value();
            let (group, kind) = parse_key(&kind_key);
            let kk = KindKind::new(&group, &kind);

            match key.as_str() {
                // Motion. Paging is Ctrl+F / Ctrl+B; Ctrl+D is delete, so there
                // is deliberately no half-page motion.
                "j" | "ArrowDown" if !ctrl => move_by(&h, repeat),
                "k" | "ArrowUp" if !ctrl => move_by(&h, -repeat),
                "g" if !ctrl => {
                    // A single `g` goes to the top, not `gg`. With a count, `5g`
                    // goes to the fifth row.
                    let shown = h.shown.get_untracked();
                    let i = (count.repeat() - 1).min(shown.len().saturating_sub(1));
                    if let Some(uid) = shown.get(i).cloned() {
                        scroll_cursor_into_view(h.table, i);
                        h.table.cursor.set(Some(uid));
                    }
                }
                "G" => {
                    let shown = h.shown.get_untracked();
                    if let Some(uid) = shown.last().cloned() {
                        scroll_cursor_into_view(h.table, shown.len() - 1);
                        h.table.cursor.set(Some(uid));
                    }
                }
                "f" if ctrl => move_by(&h, page_rows(&h)),
                "b" if ctrl => move_by(&h, -page_rows(&h)),
                "PageDown" => move_by(&h, page_rows(&h)),
                "PageUp" => move_by(&h, -page_rows(&h)),

                // Marks — the multi-selection.
                " " if !ctrl => {
                    // Mark, then step on, so holding Space tags a run of rows.
                    for _ in 0..repeat {
                        let Some(uid) = h.table.cursor.get_untracked() else {
                            break;
                        };
                        h.table.selected.update(|s| {
                            if !s.remove(&uid) {
                                s.insert(uid.clone());
                            }
                        });
                        h.table.last_clicked.set(Some(uid));
                        move_by(&h, 1);
                    }
                }
                "\\" if ctrl => h.table.selected.set(BTreeSet::new()),
                "a" if ctrl => {
                    let all: BTreeSet<String> = h.shown.get_untracked().into_iter().collect();
                    h.table.selected.set(all);
                }

                // Actions.
                // `!ctrl` matters: without it this would swallow Ctrl+D (delete)
                // and open the detail drawer instead.
                "Enter" | "d" if !ctrl => {
                    if let Some(t) = action_targets(&h).into_iter().next() {
                        detail.set(Some(t));
                    }
                }
                "r" => {
                    if let Some(t) = action_targets(&h).into_iter().next() {
                        tree_open.set(Some(t));
                    }
                }
                "l" => {
                    if kk.is_pod() || kk.is_workload() || kk.is_job() {
                        let aggregate = !kk.is_pod();
                        for t in action_targets(&h) {
                            open_logs(log_pods, LogTarget::from_detail(&t, aggregate));
                        }
                    }
                }
                "s" => {
                    // Pods only. A node shell has to round-trip the API to
                    // create the privileged pod first, so nodes go through the
                    // actions menu (`a`) where that flow already lives.
                    if kk.is_pod() {
                        if let Some(t) = action_targets(&h).into_iter().next() {
                            exec_open.set(Some(ExecTarget {
                                namespace: t.namespace.clone().unwrap_or_default(),
                                pod: t.name.clone(),
                                container: None,
                                pending: false,
                                node_shell: false,
                                image: String::new(),
                            }));
                        }
                    }
                }
                // YAML lives in a detail tab, so this opens the drawer already
                // switched to it.
                "y" if !ctrl => {
                    if let Some(t) = action_targets(&h).into_iter().next() {
                        requested_tab.set(Some(DetailTab::Yaml));
                        detail.set(Some(t));
                    }
                }
                "c" if ctrl => {
                    let names: Vec<String> =
                        action_targets(&h).into_iter().map(|t| t.name).collect();
                    if !names.is_empty() {
                        copy_to_clipboard(&names.join("\n"));
                        show_toast(toast, "Copied to clipboard", ToastKind::Ok);
                    }
                }
                "a" if !ctrl => open_menu_at_cursor(&h, ctx_menu),

                // Destructive: Ctrl+D deletes, Ctrl+K kills (force, no grace
                // period). Both still route through the confirm dialog — `kill`
                // only preselects `force`.
                "d" | "k" if ctrl => {
                    let kill = key == "k";
                    let targets = action_targets(&h);
                    if !targets.is_empty() {
                        let n = targets.len();
                        let verb = if kill { "Force delete" } else { "Delete" };
                        let message = if n == 1 {
                            format!("{verb} this resource?")
                        } else {
                            format!("{verb} {n} resources?")
                        };
                        let selected = h.table.selected;
                        ask_delete(delete_confirm, message, move |force, propagation| {
                            crate::app::events::fire_action_with(
                                toast,
                                "delete",
                                &targets,
                                delete_extra(force || kill, propagation),
                            );
                            selected.set(BTreeSet::new());
                        });
                    }
                }

                // Sorting — Shift+<letter>, matched against this kind's live
                // columns. Last arm, so it can't shadow `G`, `K`, `N`, `E`.
                other
                    if !ctrl
                        && other.chars().count() == 1
                        && other.chars().all(|c| c.is_ascii_uppercase()) =>
                {
                    let letter = other.chars().next().unwrap();
                    let columns = h.columns.get_untracked();
                    let Some(next) = sort_column_for_letter(&columns, letter) else {
                        return;
                    };
                    h.table.sort.update(|(current, asc)| {
                        // Re-pressing the active column's key reverses it.
                        if *current == next {
                            *asc = !*asc;
                        } else {
                            *current = next;
                            *asc = true;
                        }
                    });
                }
                _ => return,
            }
            e.prevent_default();
        });
        on_cleanup(move || handle.remove());
    });
}

/// Open the context menu on the cursor row, anchored where that row is drawn.
///
/// The cursor row is frequently outside the rendered window, so there is no
/// element to measure — the anchor is derived from the scroll offset and row
/// height the same way [`scroll_cursor_into_view`] derives its target.
fn open_menu_at_cursor(h: &TableKeyHandle, ctx_menu: RwSignal<Option<CtxMenu>>) {
    let Some(uid) = h.table.cursor.get_untracked() else {
        return;
    };
    let Some(row) = h.table.rows.with_untracked(|rows| rows.get(&uid).cloned()) else {
        return;
    };
    let target = DetailTarget {
        key: h.kind_key.get_value(),
        namespace: row.namespace.clone(),
        name: row.name.clone(),
    };
    let (x, y) = cursor_anchor(h, &uid);
    ctx_menu.set(Some(CtxMenu {
        x,
        y,
        target,
        node: h.node_for.run(uid.clone()),
        uid,
    }));
}

/// Viewport coordinates of the cursor row, for anchoring the menu.
fn cursor_anchor(h: &TableKeyHandle, uid: &str) -> (i32, i32) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(wrap) = h.table.table_ref.try_get_untracked().flatten() {
            let index = h
                .shown
                .get_untracked()
                .iter()
                .position(|u| u == uid)
                .unwrap_or(0) as f64;
            let row_h = h.table.row_h.get_untracked().max(1.0);
            let rect = wrap.get_bounding_client_rect();
            let y = rect.top() + (index * row_h - h.table.scroll_top.get_untracked()) + row_h;
            return (rect.left().round() as i32 + 24, y.round() as i32);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (h, uid);
    (0, 0)
}

/// Installs the desktop key dispatcher and renders the `{count}g` indicator —
/// vim's `showcmd`, so a pending chord is never invisible.
///
/// A component rather than a plain hook for two reasons: `use_navigate` (behind
/// the `g h` / `g s` / `g w` chords) panics unless it runs under a `<Router>`,
/// and mounting it inside the desktop branch keeps the entire keymap off the
/// mobile tree, which has no keyboard bindings.
#[component]
pub(crate) fn KeyLayer() -> impl IntoView {
    use_key_dispatch();
    let pending = expect_context::<PendingKeys>().0;
    view! {
        {move || {
            let p = pending.get();
            (!p.is_empty()).then(|| view! { <div class="showcmd">{p.display()}</div> })
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_accumulates_left_to_right() {
        let mut p = Pending::default();
        assert!(p.push_digit(1));
        assert!(p.push_digit(2));
        assert_eq!(p.count, 12);
        assert_eq!(p.repeat(), 12);
    }

    #[test]
    fn leading_zero_is_not_a_count() {
        let mut p = Pending::default();
        assert!(!p.push_digit(0));
        assert_eq!(p.count, 0);
        // ...but a zero after a digit is.
        assert!(p.push_digit(1));
        assert!(p.push_digit(0));
        assert_eq!(p.count, 10);
    }

    #[test]
    fn repeat_defaults_to_one() {
        assert_eq!(Pending::default().repeat(), 1);
    }

    #[test]
    fn count_saturates() {
        let mut p = Pending::default();
        for _ in 0..12 {
            p.push_digit(9);
        }
        assert_eq!(p.count, MAX_COUNT);
    }

    #[test]
    fn display_shows_the_pending_count_only() {
        let p = Pending { count: 5 };
        assert_eq!(p.display(), "5");
        assert!(!p.is_empty());
        assert!(Pending::default().is_empty());
        assert_eq!(Pending::default().display(), "");
    }

    /// `Ctrl+<n>` counts the sidebar's Favorites section, which is catalog
    /// order — not the sorted order the pins are persisted in. If these two
    /// diverged, the number on screen would open a different kind.
    #[test]
    fn favorites_are_numbered_in_catalog_order() {
        use crate::app::state::pinned_in_catalog_order;
        use roder_core::{Category, ResourceKind};

        let kind = |key: &str, name: &str| ResourceKind {
            key: key.to_string(),
            group: String::new(),
            version: "v1".to_string(),
            kind: name.to_string(),
            plural: name.to_lowercase(),
            namespaced: true,
            category: Category::Cluster,
        };
        // Catalog order deliberately disagrees with sorted-key order.
        let catalog = vec![
            kind("/v1/Pod", "Pod"),
            kind("apps/v1/Deployment", "Deployment"),
            kind("/v1/Service", "Service"),
        ];
        let pinned: std::collections::HashSet<String> =
            ["/v1/Service".to_string(), "/v1/Pod".to_string()]
                .into_iter()
                .collect();

        let ordered = pinned_in_catalog_order(&catalog, &pinned);
        let names: Vec<&str> = ordered.iter().map(|k| k.kind.as_str()).collect();
        assert_eq!(
            names,
            ["Pod", "Service"],
            "Ctrl+1 must be Pod because the catalog lists it first, even though \
             \"/v1/Pod\" sorts after \"/v1/Service\""
        );
    }

    fn cols(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    /// The sort keys users actually have in their fingers, against the real Pod
    /// columns.
    #[test]
    fn shift_letter_selects_the_expected_columns() {
        let columns = cols(&["Namespace", "Name", "Ready", "Status", "Restarts", "Age"]);
        assert_eq!(
            sort_column_for_letter(&columns, 'S'),
            Some(SortKey::Cell(3)),
            "Shift+S should sort by Status"
        );
        assert_eq!(
            sort_column_for_letter(&columns, 'R'),
            Some(SortKey::Cell(2)),
            "Shift+R should sort by Ready, the first R column"
        );
        assert_eq!(sort_column_for_letter(&columns, 'A'), Some(SortKey::Age));
        // Namespace precedes Name, so N resolves to the namespace column.
        assert_eq!(
            sort_column_for_letter(&columns, 'N'),
            Some(SortKey::Namespace)
        );
    }

    #[test]
    fn shift_letter_is_case_insensitive_and_can_miss() {
        let columns = cols(&["Namespace", "Name", "Age"]);
        assert_eq!(
            sort_column_for_letter(&columns, 'a'),
            sort_column_for_letter(&columns, 'A')
        );
        // No column starts with Q, so the key falls through unhandled.
        assert_eq!(sort_column_for_letter(&columns, 'Q'), None);
        assert_eq!(sort_column_for_letter(&[], 'A'), None);
    }

    /// CRD columns come from `additionalPrinterColumns`, so the letter has to
    /// resolve against whatever a kind actually declares.
    #[test]
    fn shift_letter_extends_to_crd_columns() {
        let columns = cols(&["Namespace", "Name", "Suspended", "Revision", "Age"]);
        assert_eq!(
            sort_column_for_letter(&columns, 'S'),
            Some(SortKey::Cell(2))
        );
        assert_eq!(
            sort_column_for_letter(&columns, 'R'),
            Some(SortKey::Cell(3))
        );
    }

    /// The help overlay is generated from this table, so a typo'd group would
    /// silently drop a row out of the rendered help.
    #[test]
    fn every_binding_lands_in_a_rendered_group() {
        for binding in BINDINGS {
            assert!(
                Group::ORDER.contains(&binding.group),
                "binding {:?} has a group missing from Group::ORDER",
                binding.keys
            );
            assert!(
                !binding.keys.is_empty(),
                "binding {} has no keys",
                binding.label
            );
        }
    }
}

//! ⌘K command palette: fuzzy search across resources with advanced filters and autocomplete.
//!
//! Supports:
//! - Free text: searches resource names
//! - `in:<kind>`: filter by resource kind (e.g., `in:pods`, `in:deployments`)
//! - `ns:<namespace>`: filter by namespace (e.g., `ns:default`, `ns:kube-system`)
//! - Combinations: `nginx in:pods ns:default` finds nginx pods in default namespace
//! - Autocomplete: Shows suggestions when typing `in:` or `ns:`
//! - Fuzzy matching: "pods" matches "PodMonitor", "monit" matches "monitoring"

use leptos::prelude::*;
use leptos::task::spawn_local;
use roder_core::{ResourceKind, ResourceRow};
use std::collections::HashMap;

use crate::app::events::{apply_event, UidSet};
use crate::app::state::{Catalog, DetailTarget, PaletteOpen, ResourceFilter};
use crate::data;

/// Result of a fuzzy match, containing matched positions for highlighting.
#[derive(Clone, Debug)]
struct FuzzyMatch {
    /// Positions of matched characters in the original string.
    positions: Vec<usize>,
    /// Match score (higher is better).
    score: i32,
}

/// Perform fuzzy matching: checks if `pattern` characters appear in `text` in order.
/// Returns match positions and score if successful.
fn fuzzy_match(pattern: &str, text: &str) -> Option<FuzzyMatch> {
    let pattern = pattern.to_lowercase();
    let text_lower = text.to_lowercase();
    let pattern_chars: Vec<char> = pattern.chars().collect();
    let text_chars: Vec<char> = text_lower.chars().collect();

    if pattern_chars.is_empty() {
        return Some(FuzzyMatch {
            positions: vec![],
            score: 0,
        });
    }

    let mut positions = Vec::new();
    let mut pattern_idx = 0;
    let mut score = 0;
    let mut last_match_idx = None;

    for (idx, &ch) in text_chars.iter().enumerate() {
        if pattern_idx < pattern_chars.len() && ch == pattern_chars[pattern_idx] {
            positions.push(idx);

            // Bonus for consecutive matches
            if let Some(last) = last_match_idx {
                if idx == last + 1 {
                    score += 10;
                }
            }

            // Bonus for matching at word boundaries
            if idx == 0
                || text_chars[idx - 1] == '_'
                || text_chars[idx - 1] == '-'
                || text_chars[idx - 1] == ' '
            {
                score += 15;
            }

            // Bonus for case-sensitive match
            if text.chars().nth(idx) == Some(pattern_chars[pattern_idx]) {
                score += 5;
            }

            score += 1;
            last_match_idx = Some(idx);
            pattern_idx += 1;
        }
    }

    if pattern_idx == pattern_chars.len() {
        Some(FuzzyMatch { positions, score })
    } else {
        None
    }
}

/// Segments of text for highlighting: either matched or unmatched.
#[derive(Clone)]
enum TextSegment {
    Matched(String),
    Unmatched(String),
}

/// Split text into segments based on match positions.
fn highlight_text(text: &str, positions: &[usize]) -> Vec<TextSegment> {
    if positions.is_empty() {
        return vec![TextSegment::Unmatched(text.to_string())];
    }

    let mut segments = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut current_segment = String::new();
    let mut in_match = false;
    let mut pos_idx = 0;

    for (idx, ch) in chars.iter().enumerate() {
        let is_match = pos_idx < positions.len() && positions[pos_idx] == idx;

        if is_match != in_match {
            // Transition: save current segment and start new one
            if !current_segment.is_empty() {
                if in_match {
                    segments.push(TextSegment::Matched(current_segment));
                } else {
                    segments.push(TextSegment::Unmatched(current_segment));
                }
                current_segment = String::new();
            }
            in_match = is_match;
        }

        if is_match {
            pos_idx += 1;
        }

        current_segment.push(*ch);
    }

    // Don't forget the last segment
    if !current_segment.is_empty() {
        if in_match {
            segments.push(TextSegment::Matched(current_segment));
        } else {
            segments.push(TextSegment::Unmatched(current_segment));
        }
    }

    segments
}

/// Component that renders text with fuzzy match highlighting.
#[component]
fn HighlightedText(
    text: impl Into<Signal<String>> + 'static,
    positions: impl Into<Signal<Vec<usize>>> + 'static,
) -> impl IntoView {
    let text = text.into();
    let positions = positions.into();

    let segments = move || {
        let text = text.get();
        let positions = positions.get();
        highlight_text(&text, &positions)
    };

    view! {
        {move || {
            segments().into_iter().map(|seg| {
                match seg {
                    TextSegment::Matched(s) => view! { <span class="highlight">{s}</span> }.into_any(),
                    TextSegment::Unmatched(s) => view! { <span>{s}</span> }.into_any(),
                }
            }).collect_view()
        }}
    }
}

/// Parsed search query with filters.
#[derive(Clone, Default, PartialEq)]
struct ParsedQuery {
    /// Free text to search in resource names.
    text: String,
    /// Namespace filters (from `ns:<namespace>`).
    namespaces: Vec<String>,
    /// Resource kind filters (from `in:<kind>`).
    kinds: Vec<String>,
    /// Current incomplete token being typed (for autocomplete).
    incomplete_in: Option<String>,
    incomplete_ns: Option<String>,
}

impl ParsedQuery {
    fn parse(query: &str) -> Self {
        let mut result = Self::default();
        let mut text_parts = Vec::new();
        let parts: Vec<&str> = query.split_whitespace().collect();

        // Check if query ends with whitespace (token is complete)
        let ends_with_space = query.ends_with(char::is_whitespace);

        for (i, part) in parts.iter().enumerate() {
            let is_last = i == parts.len() - 1;

            if let Some(value) = part.strip_prefix("in:") {
                let value = value.to_lowercase();
                if value.is_empty() && is_last && !ends_with_space {
                    result.incomplete_in = Some(String::new());
                } else if is_last && !ends_with_space {
                    result.incomplete_in = Some(value.clone());
                } else {
                    result.kinds.push(value);
                }
            } else if let Some(value) = part.strip_prefix("ns:") {
                let value = value.to_lowercase();
                if value.is_empty() && is_last && !ends_with_space {
                    result.incomplete_ns = Some(String::new());
                } else if is_last && !ends_with_space {
                    result.incomplete_ns = Some(value.clone());
                } else {
                    result.namespaces.push(value);
                }
            } else {
                text_parts.push(*part);
            }
        }

        result.text = text_parts
            .into_iter()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        result
    }
}

#[component]
pub(crate) fn CommandPalette() -> impl IntoView {
    let palette_open = expect_context::<PaletteOpen>().0;
    let catalog = expect_context::<Catalog>().0;
    let selected_kind = expect_context::<RwSignal<Option<ResourceKind>>>();
    let selected_ns = expect_context::<RwSignal<Option<String>>>();
    let resource_filter = expect_context::<ResourceFilter>().0;
    let detail = expect_context::<RwSignal<Option<DetailTarget>>>();
    let query = RwSignal::new(String::new());
    let autocomplete_index = RwSignal::new(0usize);
    let namespaces = RwSignal::new(Vec::<String>::new());
    let input_ref = NodeRef::<leptos::html::Input>::new();

    // Resource rows for each target kind (supports multiple kinds)
    let all_rows: RwSignal<HashMap<String, HashMap<String, ResourceRow>>> =
        RwSignal::new(HashMap::new());
    let entering: UidSet = RwSignal::new(std::collections::BTreeSet::new());
    let removing: UidSet = RwSignal::new(std::collections::BTreeSet::new());

    // Fetch namespaces on mount
    Effect::new(move |_| {
        spawn_local(async move {
            if let Ok(ns) = data::fetch_json::<Vec<String>>("/api/namespaces").await {
                namespaces.set(ns);
            }
        });
    });

    // Reset query and focus input when palette opens
    Effect::new(move |_| {
        if palette_open.get() {
            query.set(String::new());
            autocomplete_index.set(0);
            // Focus the input when palette opens
            if let Some(input) = input_ref.get_untracked() {
                let _ = input.focus();
            }
        }
    });

    // Parse the query
    let parsed = Memo::new(move |_| {
        let q = query.get();
        ParsedQuery::parse(&q)
    });

    // Generate autocomplete suggestions
    let autocomplete_suggestions = Memo::new(move |_| {
        let p = parsed.get();
        let kinds = catalog.get();
        let ns_list = namespaces.get();
        let mut suggestions = Vec::new();

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(
            &format!(
                "autocomplete_suggestions: incomplete_in={:?} incomplete_ns={:?}",
                p.incomplete_in, p.incomplete_ns
            )
            .into(),
        );

        // Handle `in:` for resource kinds
        if let Some(incomplete) = &p.incomplete_in {
            for kind in kinds.iter() {
                // Match against plural form (what kubectl uses)
                if let Some(m) = fuzzy_match(incomplete, &kind.plural) {
                    suggestions.push(AutocompleteSuggestion {
                        text: kind.plural.clone(),
                        kind: AutocompleteKind::ResourceType(kind.kind.clone()),
                        prefix: "in:".to_string(),
                        match_positions: m.positions,
                        score: m.score,
                    });
                }
            }
        }

        // Handle `ns:` for namespaces
        if let Some(incomplete) = &p.incomplete_ns {
            for ns in ns_list.iter() {
                if let Some(m) = fuzzy_match(incomplete, ns) {
                    suggestions.push(AutocompleteSuggestion {
                        text: ns.clone(),
                        kind: AutocompleteKind::Namespace,
                        prefix: "ns:".to_string(),
                        match_positions: m.positions,
                        score: m.score,
                    });
                }
            }
        }

        // Sort by score (descending)
        suggestions.sort_by_key(|s| std::cmp::Reverse(s.score));
        suggestions.truncate(10);

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&format!("Total suggestions: {}", suggestions.len()).into());

        if suggestions.is_empty() {
            None
        } else {
            Some(suggestions)
        }
    });

    // Reset autocomplete index when suggestions change
    Effect::new(move |_| {
        autocomplete_suggestions.get();
        autocomplete_index.set(0);
    });

    // Determine all target resource kinds for searching
    let target_kinds = Memo::new(move |_| {
        let p = parsed.get();
        let kinds = catalog.get();
        let mut result = Vec::new();

        // If `in:<kind>` filters are specified, find all matching kinds
        for kind_name in &p.kinds {
            // Try exact match on plural first
            if let Some(k) = kinds.iter().find(|k| k.plural.to_lowercase() == *kind_name) {
                result.push(k.clone());
                continue;
            }
            // Try exact match on singular
            if let Some(k) = kinds.iter().find(|k| k.kind.to_lowercase() == *kind_name) {
                result.push(k.clone());
                continue;
            }
            // Try fuzzy match on plural
            if let Some(k) = kinds
                .iter()
                .find(|k| fuzzy_match(kind_name, &k.plural).is_some())
            {
                result.push(k.clone());
            }
        }

        // If no kinds specified, use the currently selected kind
        if result.is_empty() {
            if let Some(k) = selected_kind.get() {
                result.push(k);
            }
        }

        result
    });

    // Subscribe to resources for all target kinds
    Effect::new(move |_| {
        let kinds = target_kinds.get();
        let p = parsed.get();

        // Clear all rows when targets change
        all_rows.set(HashMap::new());

        // Subscribe to each kind
        for kind in kinds {
            let kind_key = kind.key.clone();
            let rows_for_kind = RwSignal::new(HashMap::<String, ResourceRow>::new());

            // Determine namespace for this subscription
            let ns = p.namespaces.first().cloned().or_else(|| selected_ns.get());
            let url = data::watch_url(&kind_key, ns.as_deref(), None);

            // Create subscription for this kind
            let rows_copy = rows_for_kind;
            let all_rows_copy = all_rows;

            Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
                let url = url.clone();
                let kind_key_inner = kind_key.clone();
                data::subscribe(&url, move |ev| {
                    apply_event(rows_copy, entering, removing, ev);
                    // Sync to all_rows
                    all_rows_copy.update(|ar| {
                        ar.insert(kind_key_inner.clone(), rows_copy.get_untracked());
                    });
                })
            });
        }
    });

    // Filter and sort matching resources across all kinds
    let matches = Memo::new(move |_| {
        let p = parsed.get();
        let kinds = target_kinds.get();
        let catalog_data = catalog.get();

        // If no kinds are specified, show resource kinds
        if kinds.is_empty() {
            return Some(SearchResults::Kinds(filter_kinds(&catalog_data, &p.text)));
        }

        // Otherwise, search resources across all kinds
        let all_rows_data = all_rows.get();
        let mut all_resources = Vec::new();

        for kind in &kinds {
            if let Some(rows) = all_rows_data.get(&kind.key) {
                let resources = filter_resources(rows, &p, kind);
                all_resources.extend(resources);
            }
        }

        // Sort by name (since scores are already handled per-kind)
        all_resources.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        all_resources.truncate(100);

        Some(SearchResults::Resources(all_resources))
    });

    let choose_kind = move |k: ResourceKind| {
        selected_kind.set(Some(k));
        palette_open.set(false);
    };

    let choose_resource = move |r: ResourceRow, kind: ResourceKind| {
        let key = kind.key.clone();
        selected_kind.set(Some(kind));
        detail.set(Some(DetailTarget {
            key,
            namespace: r.namespace,
            name: r.name,
        }));
        palette_open.set(false);
    };

    // Apply search filters without opening a resource
    let apply_search = move || {
        let p = parsed.get();
        let kinds = catalog.get();

        // If multiple kinds are specified, save to session storage and navigate to search view
        if p.kinds.len() > 1 {
            #[cfg(target_arch = "wasm32")]
            {
                use crate::app::state::MultiKindSearch;

                let search_query = MultiKindSearch {
                    kinds: p.kinds.clone(),
                    namespaces: p.namespaces.clone(),
                    text: p.text.clone(),
                };

                if let Some(storage) = web_sys::window()
                    .and_then(|w| w.session_storage().ok())
                    .flatten()
                {
                    if let Ok(json) = serde_json::to_string(&search_query) {
                        let _ = storage.set_item("roder_search_query", &json);
                    }
                }

                // Set the resource filter before navigating
                resource_filter.set(p.text.clone());

                // Navigate to search view
                if let Some(window) = web_sys::window() {
                    let _ = window.location().set_href("/search");
                }
            }

            palette_open.set(false);
            return;
        }

        // Apply kind filter if exactly one kind is specified
        if p.kinds.len() == 1 {
            let kind_name = &p.kinds[0];
            // Try exact match on plural first
            if let Some(k) = kinds.iter().find(|k| k.plural.to_lowercase() == *kind_name) {
                selected_kind.set(Some(k.clone()));
            }
            // Try exact match on singular
            else if let Some(k) = kinds.iter().find(|k| k.kind.to_lowercase() == *kind_name) {
                selected_kind.set(Some(k.clone()));
            }
            // Try fuzzy match on plural
            else if let Some(k) = kinds
                .iter()
                .find(|k| fuzzy_match(kind_name, &k.plural).is_some())
            {
                selected_kind.set(Some(k.clone()));
            }
        }

        // Apply namespace filter if specified
        if let Some(ns) = p.namespaces.first() {
            selected_ns.set(Some(ns.clone()));
        }

        // Apply text filter
        resource_filter.set(p.text.clone());

        palette_open.set(false);
    };

    let apply_autocomplete = move |suggestion: AutocompleteSuggestion| {
        let q = query.get();
        let parts: Vec<&str> = q.split_whitespace().collect();
        let prefix = suggestion.prefix.clone();

        // Find and replace the last incomplete token with matching prefix
        let mut new_parts = Vec::new();
        let mut replaced = false;

        for part in parts.iter().rev() {
            if part.starts_with(&prefix) && !replaced {
                new_parts.push(format!("{}{}", prefix, suggestion.text));
                replaced = true;
            } else {
                new_parts.push(part.to_string());
            }
        }

        new_parts.reverse();
        // Add trailing space to mark the token as complete
        let new_query = new_parts.join(" ") + " ";
        query.set(new_query);
    };

    let handle_keydown = move |e: leptos::ev::KeyboardEvent| {
        match e.key().as_str() {
            "Enter" => {
                // Check if we have autocomplete suggestions
                if let Some(suggestions) = autocomplete_suggestions.get() {
                    if !suggestions.is_empty() {
                        let idx = autocomplete_index.get();
                        if let Some(suggestion) = suggestions.get(idx) {
                            apply_autocomplete(suggestion.clone());
                            e.prevent_default();
                            return;
                        }
                    }
                }

                // Otherwise, apply the search filters and close the palette
                apply_search();
                e.prevent_default();
            }
            "ArrowDown" => {
                if let Some(suggestions) = autocomplete_suggestions.get() {
                    if !suggestions.is_empty() {
                        let idx = autocomplete_index.get();
                        autocomplete_index.set((idx + 1) % suggestions.len());
                        e.prevent_default();
                    }
                }
            }
            "ArrowUp" => {
                if let Some(suggestions) = autocomplete_suggestions.get() {
                    if !suggestions.is_empty() {
                        let idx = autocomplete_index.get();
                        autocomplete_index.set(if idx == 0 {
                            suggestions.len() - 1
                        } else {
                            idx - 1
                        });
                        e.prevent_default();
                    }
                }
            }
            "Tab" => {
                // Accept autocomplete suggestion
                if let Some(suggestions) = autocomplete_suggestions.get() {
                    if !suggestions.is_empty() {
                        let idx = autocomplete_index.get();
                        if let Some(suggestion) = suggestions.get(idx) {
                            apply_autocomplete(suggestion.clone());
                            e.prevent_default();
                        }
                    }
                }
            }
            _ => {}
        }
    };

    view! {
        <Show when=move || palette_open.get() fallback=|| ()>
            <div class="palette-scrim" on:click=move |_| palette_open.set(false)></div>
            <div class="palette">
                <div class="palette-input-wrap">
                    <input class="palette-input" node_ref=input_ref autofocus=true
                        placeholder="Search resources... (in:kind ns:namespace)"
                        prop:value=move || query.get()
                        on:input=move |e| query.set(event_target_value(&e))
                        on:keydown=handle_keydown />

                    // Autocomplete dropdown
                    <Show when=move || autocomplete_suggestions.get().map(|s| !s.is_empty()).unwrap_or(false)>
                        <ul class="autocomplete-list">
                            <For
                                each=move || autocomplete_suggestions.get().unwrap_or_default()
                                key=|s| format!("{:?}-{}-{:?}", s.kind, s.text, s.match_positions)
                                let:suggestion
                            >
                                {
                                    let idx = autocomplete_suggestions.get()
                                        .unwrap_or_default()
                                        .iter()
                                        .position(|s| s == &suggestion)
                                        .unwrap_or(0);
                                    let is_selected = move || autocomplete_index.get() == idx;
                                    let apply = apply_autocomplete;
                                    let sugg = suggestion.clone();

                                    view! {
                                        <li class="autocomplete-item"
                                            class:selected=is_selected
                                            on:click=move |_| apply(sugg.clone())>
                                            <span class="autocomplete-text">
                                                {suggestion.prefix.clone()}
                                                <HighlightedText text=suggestion.text.clone() positions=suggestion.match_positions.clone() />
                                            </span>
                                            <span class="autocomplete-type">
                                                {match &suggestion.kind {
                                                    AutocompleteKind::Namespace => "namespace".to_string(),
                                                    AutocompleteKind::ResourceType(t) => t.clone(),
                                                }}
                                            </span>
                                        </li>
                                    }
                                }
                            </For>
                        </ul>
                    </Show>
                </div>

                <ul class="palette-list">
                    {move || match matches.get() {
                        None => view! { <li class="palette-item muted">"Loading..."</li> }.into_any(),
                        Some(SearchResults::Kinds(kinds)) => {
                            view! {
                                <For each=move || kinds.clone() key=|(k, _)| k.key.clone() let:item>
                                    {
                                        let (k, positions) = item;
                                        let choose = choose_kind;
                                        let kind = k.clone();
                                        let group = if k.group.is_empty() { "core".to_string() } else { k.group.clone() };
                                        view! {
                                            <li class="palette-item" on:click=move |_| choose(kind.clone())>
                                                <span class="pi-kind"><HighlightedText text=k.kind.clone() positions=positions.clone() /></span>
                                                <span class="pi-group">{group}</span>
                                                <span class="pi-cat">{k.category.label()}</span>
                                            </li>
                                        }
                                    }
                                </For>
                            }.into_any()
                        }
                        Some(SearchResults::Resources(resources)) => {
                            view! {
                                <For each=move || resources.clone() key=|(_, r, _)| r.uid.clone() let:item>
                                    {
                                        let (kind, r, positions) = item;
                                        let choose = choose_resource;
                                        let k = kind.clone();
                                        let row = r.clone();
                                        let status_class = match r.status {
                                            roder_core::RowStatus::Ok => "status-ok",
                                            roder_core::RowStatus::Warn => "status-warn",
                                            roder_core::RowStatus::Error => "status-error",
                                            _ => "",
                                        };
                                        view! {
                                            <li class="palette-item" on:click=move |_| choose(row.clone(), k.clone())>
                                                <span class="pi-name"><HighlightedText text=r.name.clone() positions=positions.clone() /></span>
                                                {r.namespace.map(|ns| view! { <span class="pi-ns">{ns}</span> })}
                                                <span class=format!("pi-status {}", status_class)>
                                                    {r.cells.get(1).cloned().unwrap_or_default()}
                                                </span>
                                            </li>
                                        }
                                    }
                                </For>
                            }.into_any()
                        }
                    }}
                </ul>
                <div class="palette-hints">
                    <span class="hint"><kbd>"in:"</kbd>" kind"</span>
                    <span class="hint"><kbd>"ns:"</kbd>" namespace"</span>
                    <span class="hint"><kbd>"↑↓"</kbd>" navigate"</span>
                    <span class="hint"><kbd>"tab"</kbd>" accept"</span>
                    <span class="hint"><kbd>"esc"</kbd>" close"</span>
                </div>
            </div>
        </Show>
    }
}

#[derive(Clone, PartialEq)]
enum SearchResults {
    Kinds(Vec<(ResourceKind, Vec<usize>)>),
    Resources(Vec<(ResourceKind, ResourceRow, Vec<usize>)>),
}

#[derive(Clone, PartialEq, Debug)]
enum AutocompleteKind {
    Namespace,
    ResourceType(String),
}

#[derive(Clone, Debug)]
struct AutocompleteSuggestion {
    text: String,
    kind: AutocompleteKind,
    prefix: String,
    match_positions: Vec<usize>,
    score: i32,
}

impl PartialEq for AutocompleteSuggestion {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text && self.kind == other.kind && self.prefix == other.prefix
    }
}

fn filter_kinds(catalog: &[ResourceKind], query: &str) -> Vec<(ResourceKind, Vec<usize>)> {
    let mut v: Vec<(ResourceKind, Vec<usize>, i32)> = catalog
        .iter()
        .filter_map(|k| {
            if query.is_empty() {
                return Some((k.clone(), vec![], 0));
            }
            // Match against plural form
            let plural_match = fuzzy_match(query, &k.plural);

            plural_match.map(|m| (k.clone(), m.positions, m.score))
        })
        .collect();

    // Sort by score (descending)
    v.sort_by_key(|(_, _, score)| std::cmp::Reverse(*score));
    v.truncate(60);
    v.into_iter().map(|(k, pos, _)| (k, pos)).collect()
}

fn filter_resources(
    rows: &HashMap<String, ResourceRow>,
    query: &ParsedQuery,
    kind: &ResourceKind,
) -> Vec<(ResourceKind, ResourceRow, Vec<usize>)> {
    let mut v: Vec<(ResourceKind, ResourceRow, Vec<usize>, i32)> = rows
        .values()
        .filter_map(|r| {
            // Filter by namespaces (if specified in query)
            if !query.namespaces.is_empty() {
                if let Some(ns) = &r.namespace {
                    if !query.namespaces.iter().any(|n| n == ns) {
                        return None;
                    }
                } else {
                    return None;
                }
            }

            // Fuzzy match on name
            let name_match = if query.text.is_empty() {
                Some(FuzzyMatch {
                    positions: vec![],
                    score: 0,
                })
            } else {
                fuzzy_match(&query.text, &r.name)
            };

            name_match.map(|m| (kind.clone(), r.clone(), m.positions, m.score))
        })
        .collect();

    // Sort by score (descending), then by name
    v.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| a.1.name.cmp(&b.1.name)));
    v.truncate(100);
    v.into_iter().map(|(k, r, pos, _)| (k, r, pos)).collect()
}

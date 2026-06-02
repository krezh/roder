//! Multi-kind search results view.
//!
//! Displays search results from multiple resource kinds in a unified list.
//! The search query is stored in session storage and retrieved on mount.

use leptos::prelude::*;
use roder_core::ResourceRow;
use std::collections::HashMap;

use crate::app::events::{apply_event, UidSet};
use crate::app::state::{MultiKindSearch, ResourceFilter};
use crate::app::util::color::name_color;
use crate::data;

/// Column width bounds (in `ch`).
const MIN_CH: usize = 5;
const CAP_CH: usize = 44;
const PAD_CH: usize = 3;

/// Displayed character width of a cell.
fn disp_len(s: &str) -> usize {
    s.chars().count() + s.matches('\n').count()
}

fn col_width(max_chars: usize) -> usize {
    (max_chars + PAD_CH).clamp(MIN_CH, CAP_CH)
}

/// Known minimum visual widths for columns.
fn min_width(header: &str) -> usize {
    match header {
        "Ready" | "Available" | "Completions" => 5,
        "Status" => 24,
        "Restarts" => 3,
        "CPU" => 8,
        "MEM" => 7,
        "%CPU/R" | "%CPU/L" | "%MEM/R" | "%MEM/L" => 5,
        "IP" => 15,
        "Node" => 10,
        "Phase" => 8,
        "Version" => 12,
        "Type" | "Store" => 8,
        "Kind" => 15,
        _ => 0,
    }
}

#[component]
pub(crate) fn SearchResultsView() -> impl IntoView {
    let resource_filter = expect_context::<ResourceFilter>().0;
    
    // Load search query from session storage
    let search_query = RwSignal::new(None::<MultiKindSearch>);
    
    Effect::new(move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok()).flatten() {
                if let Ok(Some(json)) = storage.get_item("roder_search_query") {
                    if let Ok(query) = serde_json::from_str::<MultiKindSearch>(&json) {
                        search_query.set(Some(query));
                    }
                }
            }
        }
    });
    
    // Resource rows for each kind
    let all_rows: RwSignal<HashMap<String, HashMap<String, ResourceRow>>> = 
        RwSignal::new(HashMap::new());
    let entering: UidSet = RwSignal::new(std::collections::BTreeSet::new());
    let removing: UidSet = RwSignal::new(std::collections::BTreeSet::new());
    
    // Subscribe to resources for all kinds in the search query
    Effect::new(move |_| {
        let query = search_query.get();
        let Some(query) = query else { return };
        
        // Clear all rows when query changes
        all_rows.set(HashMap::new());
        
        // Get catalog to resolve kind names
        let catalog = expect_context::<crate::app::state::Catalog>().0;
        let kinds = catalog.get();
        
        // Subscribe to each kind
        for kind_name in &query.kinds {
            // Find the kind in catalog
            let kind = kinds.iter().find(|k| {
                k.plural.to_lowercase() == kind_name.to_lowercase() ||
                k.kind.to_lowercase() == kind_name.to_lowercase()
            });
            
            let Some(kind) = kind else { continue };
            
            let kind_key = kind.key.clone();
            let rows_for_kind = RwSignal::new(HashMap::<String, ResourceRow>::new());
            let entering_clone = entering.clone();
            let removing_clone = removing.clone();
            
            // Determine namespace for this subscription
            let ns = query.namespaces.first().cloned();
            let url = data::watch_url(&kind_key, ns.as_deref(), None);
            
            // Create subscription for this kind
            let rows_copy = rows_for_kind.clone();
            let all_rows_copy = all_rows.clone();
            
            Effect::new(move |_prev: Option<Option<data::SseHandle>>| {
                let url = url.clone();
                let kind_key_inner = kind_key.clone();
                data::subscribe(&url, move |ev| {
                    apply_event(rows_copy, entering_clone.clone(), removing_clone.clone(), ev);
                    // Sync to all_rows
                    all_rows_copy.update(|ar| {
                        ar.insert(kind_key_inner.clone(), rows_copy.get_untracked());
                    });
                })
            });
        }
    });
    
    // Combine all rows with kind information
    let combined_rows = Memo::new(move |_| {
        let query = search_query.get();
        let Some(query) = query else { return Vec::new() };
        
        let all_rows_data = all_rows.get();
        let catalog = expect_context::<crate::app::state::Catalog>().0;
        let kinds = catalog.get();
        
        let mut combined = Vec::new();
        
        for kind_name in &query.kinds {
            let kind = kinds.iter().find(|k| {
                k.plural.to_lowercase() == kind_name.to_lowercase() ||
                k.kind.to_lowercase() == kind_name.to_lowercase()
            });
            
            let Some(kind) = kind else { continue };
            
            if let Some(rows) = all_rows_data.get(&kind.key) {
                for row in rows.values() {
                    combined.push((kind.clone(), row.clone()));
                }
            }
        }
        
        combined
    });
    
    // Filter and sort
    let shown_uids = Memo::new(move |_| {
        let filter_text = resource_filter.get().to_lowercase();
        let rows = combined_rows.get();
        
        let mut filtered: Vec<_> = rows
            .into_iter()
            .filter(|(_, r)| {
                if filter_text.is_empty() {
                    true
                } else {
                    r.name.to_lowercase().contains(&filter_text)
                }
            })
            .collect();
        
        // Sort by name
        filtered.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        
        filtered
    });
    
    // Column definitions (with Kind column added)
    let cols = vec![
        "Kind".to_string(),
        "Ready".to_string(),
        "Status".to_string(),
        "Restarts".to_string(),
        "CPU".to_string(),
        "%CPU/R".to_string(),
        "%CPU/L".to_string(),
        "MEM".to_string(),
        "%MEM/R".to_string(),
        "%MEM/L".to_string(),
        "IP".to_string(),
        "Node".to_string(),
    ];
    let ncols = cols.len();
    let cols_for_w = cols.clone();
    
    // Measure column widths
    let grid_template = RwSignal::new(String::new());
    Effect::new(move |_| {
        let n = shown_uids.with(|v| v.len());
        if n == 0 {
            return;
        }
        
        let mut name_w = "Name".len();
        let mut cell_w: Vec<usize> = cols_for_w.iter().map(|c| c.len().max(min_width(c))).collect();
        
        shown_uids.with(|m| {
            for (_, r) in m {
                name_w = name_w.max(r.name.chars().count());
                for i in 0..ncols {
                    // Skip Kind column (index 0) as it's handled separately
                    if i == 0 {
                        continue;
                    }
                    if let Some(c) = r.cells.get(i - 1) {
                        cell_w[i] = cell_w[i].max(disp_len(c));
                    }
                }
            }
        });
        
        let mut tracks: Vec<String> = Vec::new();
        // Name column
        tracks.push(format!("{}ch", col_width(name_w + 2)));
        // Other columns
        for w in &cell_w {
            tracks.push(format!("{}ch", col_width(*w)));
        }
        
        grid_template.set(format!(
            "grid-template-columns: {} minmax(0,1fr);",
            tracks.join(" ")
        ));
    });
    
    view! {
        <div class="resource-view">
            <div class="view-head">
                <h2 class="view-title">"Search Results"</h2>
                <button class="act" on:click=move |_| {
                    // Clear search and go back
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok()).flatten() {
                            let _ = storage.remove_item("roder_search_query");
                        }
                    }
                    // Navigate back to previous view
                    #[cfg(target_arch = "wasm32")]
                    {
                        if let Some(window) = web_sys::window() {
                            let _ = window.history().and_then(|h| h.back());
                        }
                    }
                }>"Clear Search"</button>
                <input class="row-filter" placeholder="Filter results..."
                    prop:value=move || resource_filter.get()
                    on:input=move |e| resource_filter.set(event_target_value(&e)) />
                <span class="count">{move || format!("{} results", shown_uids.get().len())}</span>
            </div>
            
            <div class="table-wrap">
                <div class="grid-table" style=move || grid_template.get()>
                    <div class="grid-row head">
                        <div class="cell">"Name"</div>
                        {cols.iter().map(|c| view! { <div class="cell">{c.clone()}</div> }).collect_view()}
                    </div>
                    
                    <For
                        each=move || shown_uids.get()
                        key=|(_, r)| r.uid.clone()
                        let:item>
                    {
                        let (kind, row) = item;
                        let kind_name = kind.kind.clone();
                        let row_clone = row.clone();
                        
                        view! {
                            <div class="grid-row row">
                                <div class="cell cell-name">
                                    <div class="cw"><div class="cwi">
                                        <span class="nm" style=move || name_color(row_clone.status)>
                                            {row_clone.name.clone()}
                                        </span>
                                    </div></div>
                                </div>
                                <div class="cell">{kind_name}</div>
                                {row.cells.iter().map(|c| view! { <div class="cell">{c.clone()}</div> }).collect_view()}
                            </div>
                        }
                    }
                    </For>
                </div>
                
                {move || {
                    let results = shown_uids.get();
                    if results.is_empty() {
                        view! { <div class="empty pad">"No results found"</div> }.into_any()
                    } else {
                        ().into_any()
                    }
                }}
            </div>
        </div>
    }
}

//! Candidate filtering for the palettes: which kinds and namespaces to offer
//! for a query, and in what order. Ranking is delegated to [`super::fuzzy`].

use roder_core::ResourceKind;

use super::fuzzy::fuzzy_match;

pub(crate) fn filter_kinds(
    catalog: &[ResourceKind],
    query: &str,
) -> Vec<(ResourceKind, Vec<usize>)> {
    let mut matches: Vec<_> = catalog
        .iter()
        .filter_map(|kind| {
            if query.is_empty() {
                return Some((kind.clone(), Vec::new(), 0));
            }
            let plural = fuzzy_match(query, &kind.plural);
            let name = fuzzy_match(query, &kind.kind);
            match (plural, name) {
                (None, None) => None,
                (Some((positions, score)), None) | (None, Some((positions, score))) => {
                    Some((kind.clone(), positions, score))
                }
                (Some(a), Some(b)) => {
                    let (positions, score) = if a.1 >= b.1 { a } else { b };
                    Some((kind.clone(), positions, score))
                }
            }
        })
        .collect();
    matches.sort_by_key(|(_, _, score)| std::cmp::Reverse(*score));
    matches.truncate(60);
    matches
        .into_iter()
        .map(|(kind, positions, _)| (kind, positions))
        .collect()
}

pub(crate) fn filter_namespaces(
    namespaces: Vec<String>,
    selected: Option<String>,
    query: &str,
) -> Vec<(Option<String>, String, Vec<usize>)> {
    let mut values: Vec<Option<String>> = std::iter::once(None)
        .chain(namespaces.into_iter().map(Some))
        .collect();
    if query.is_empty() {
        if let Some(index) = selected.and_then(|selected| {
            values
                .iter()
                .position(|value| value.as_ref() == Some(&selected))
        }) {
            let active = values.remove(index);
            values.insert(1, active);
        }
    }
    let mut scored: Vec<_> = values
        .into_iter()
        .filter_map(|value| {
            let label = value.as_deref().unwrap_or("All namespaces").to_string();
            if query.is_empty() {
                Some((value, label, Vec::new(), 0))
            } else {
                fuzzy_match(query, &label)
                    .map(|(positions, score)| (value, label, positions, score))
            }
        })
        .collect();
    if !query.is_empty() {
        scored.sort_by_key(|(_, _, _, score)| std::cmp::Reverse(*score));
    }
    scored
        .into_iter()
        .map(|(value, label, positions, _)| (value, label, positions))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_filter_keeps_all_and_active_first() {
        let values =
            filter_namespaces(vec!["alpha".into(), "beta".into()], Some("beta".into()), "");
        assert_eq!(
            values
                .iter()
                .map(|value| value.0.as_deref())
                .collect::<Vec<_>>(),
            vec![None, Some("beta"), Some("alpha")]
        );
    }
}

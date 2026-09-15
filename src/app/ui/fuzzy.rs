//! Subsequence fuzzy matching, and the segment split used to highlight what
//! matched. Shared by the command palette, the namespace palette and the
//! mobile pickers so they all rank and render candidates identically.

pub(crate) fn fuzzy_match(pattern: &str, text: &str) -> Option<(Vec<usize>, i32)> {
    let pattern: Vec<char> = pattern.to_lowercase().chars().collect();
    let lowered = text.to_lowercase();
    let chars: Vec<char> = lowered.chars().collect();
    if pattern.is_empty() {
        return Some((Vec::new(), 0));
    }
    let mut positions = Vec::new();
    let mut pattern_index = 0;
    let mut score = 0;
    let mut last = None;
    for (index, character) in chars.iter().enumerate() {
        if pattern_index < pattern.len() && *character == pattern[pattern_index] {
            positions.push(index);
            if last.is_some_and(|last| index == last + 1) {
                score += 10;
            }
            if index == 0 || matches!(chars[index - 1], '_' | '-' | ' ') {
                score += 15;
            }
            if text.chars().nth(index) == Some(pattern[pattern_index]) {
                score += 5;
            }
            score += 1;
            last = Some(index);
            pattern_index += 1;
        }
    }
    (pattern_index == pattern.len()).then_some((positions, score))
}

/// Split `text` into consecutive (segment, matched) runs, so a palette can
/// highlight exactly the characters [`fuzzy_match`] consumed.
pub(crate) fn highlight(text: &str, positions: &[usize]) -> Vec<(String, bool)> {
    if positions.is_empty() {
        return vec![(text.to_string(), false)];
    }
    let positions: std::collections::HashSet<_> = positions.iter().copied().collect();
    let mut output = Vec::new();
    let mut buffer = String::new();
    let mut matched = positions.contains(&0);
    for (index, character) in text.chars().enumerate() {
        let next_matched = positions.contains(&index);
        if next_matched != matched {
            output.push((std::mem::take(&mut buffer), matched));
            matched = next_matched;
        }
        buffer.push(character);
    }
    if !buffer.is_empty() {
        output.push((buffer, matched));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_match_rewards_contiguous_word_prefixes() {
        assert!(
            fuzzy_match("dep", "Deployment").unwrap().1
                > fuzzy_match("dep", "DebugEndpoint").unwrap().1
        );
    }

    #[test]
    fn highlight_groups_matched_and_unmatched_segments() {
        assert_eq!(
            highlight("Deployment", &[0, 1, 3]),
            vec![
                ("De".into(), true),
                ("p".into(), false),
                ("l".into(), true),
                ("oyment".into(), false),
            ]
        );
    }

    #[test]
    fn fuzzy_match_rejects_out_of_order_characters() {
        assert!(fuzzy_match("pod", "Deployment").is_none());
    }
}

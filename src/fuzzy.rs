//! Small subsequence fuzzy scorer shared by the picker and `resolve`.

const SEPARATORS: &[char] = &['-', '_', '/', ':', '.', ' '];

pub struct Match {
    pub score: i64,
    /// Char indexes of `text` that matched, for highlighting.
    pub positions: Vec<usize>,
}

/// Every whitespace-separated token of `query` must match `text` as a
/// case-insensitive subsequence. Higher is better; `None` means no match.
pub fn find(query: &str, text: &str) -> Option<Match> {
    // One lowered char per char keeps positions aligned with `text`.
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    let text: Vec<char> = text.chars().map(lower).collect();
    let mut score = 0;
    let mut positions = Vec::new();
    for token in query.split_whitespace() {
        let token: Vec<char> = token.chars().map(lower).collect();
        score += score_token(&token, &text, &mut positions)?;
    }
    // Prefer shorter candidates when everything else is equal.
    positions.sort_unstable();
    positions.dedup();
    Some(Match {
        score: score - text.len() as i64 / 8,
        positions,
    })
}

pub fn score(query: &str, text: &str) -> Option<i64> {
    find(query, text).map(|m| m.score)
}

/// Greedy match of `token` in `text` starting the first char at `start`.
fn align(token: &[char], text: &[char], start: usize) -> Option<(i64, Vec<usize>)> {
    let mut score = 0;
    let mut pos = start;
    let mut prev: Option<usize> = None;
    let mut positions = Vec::with_capacity(token.len());
    for &c in token {
        let found = pos + text[pos..].iter().position(|&t| t == c)?;
        score += 16;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 8;
        }
        if found == 0 || SEPARATORS.contains(&text[found - 1]) {
            score += 10;
        }
        // Gaps between matched chars cost; where the token starts does not.
        if let Some(p) = prev {
            score -= (found - p - 1).min(8) as i64;
        }
        positions.push(found);
        prev = Some(found);
        pos = found + 1;
    }
    Some((score, positions))
}

/// Best of the greedy alignments that begin at each occurrence of the first char,
/// so `feat` prefers the contiguous `f-feat` match over a scattered `fxeat`.
fn score_token(token: &[char], text: &[char], positions: &mut Vec<usize>) -> Option<i64> {
    let first = *token.first()?;
    let best = text
        .iter()
        .enumerate()
        .filter(|&(_, &c)| c == first)
        .filter_map(|(i, _)| align(token, text, i))
        .max_by_key(|(score, _)| *score)?;
    positions.extend(best.1);
    Some(best.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert!(score("", "anything").is_some());
    }

    #[test]
    fn requires_subsequence() {
        assert!(score("pho", "pooh").is_none());
        assert!(score("poh", "pooh").is_some());
    }

    #[test]
    fn is_case_insensitive() {
        assert!(score("POOH", "pooh").is_some());
    }

    #[test]
    fn tokens_are_anded() {
        assert!(score("api main", "api:main").is_some());
        assert!(score("api dev", "api:main").is_none());
    }

    #[test]
    fn prefers_the_contiguous_alignment() {
        assert!(score("feat", "f-feat") > score("feat", "fxeat"));
        assert_eq!(find("feat", "f-feat").unwrap().positions, vec![2, 3, 4, 5]);
    }

    #[test]
    fn reports_matched_positions() {
        assert_eq!(find("ab", "xaxb").unwrap().positions, vec![1, 3]);
        assert!(find("", "abc").unwrap().positions.is_empty());
    }

    #[test]
    fn contiguous_and_boundary_matches_rank_higher() {
        assert!(score("web", "web-app") > score("web", "awesome-bed"));
        assert!(score("pooh", "pooh") > score("pooh", "pxoxoxoxh"));
    }
}

//! Small subsequence fuzzy scorer shared by the picker and `resolve`.

const SEPARATORS: &[char] = &['-', '_', '/', ':', '.', ' '];

/// Every whitespace-separated token of `query` must match `text` as a
/// case-insensitive subsequence. Higher is better; `None` means no match.
pub fn score(query: &str, text: &str) -> Option<i64> {
    let text: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let mut total = 0;
    for token in query.split_whitespace() {
        total += score_token(&token.to_lowercase().chars().collect::<Vec<_>>(), &text)?;
    }
    // Prefer shorter candidates when everything else is equal.
    Some(total - text.len() as i64 / 8)
}

fn score_token(token: &[char], text: &[char]) -> Option<i64> {
    let mut score = 0;
    let mut pos = 0;
    let mut prev: Option<usize> = None;
    for &c in token {
        let found = pos + text[pos..].iter().position(|&t| t == c)?;
        score += 16;
        if prev.is_some_and(|p| p + 1 == found) {
            score += 8;
        }
        if found == 0 || SEPARATORS.contains(&text[found - 1]) {
            score += 10;
        }
        score -= (found - prev.map_or(0, |p| p + 1)).min(8) as i64;
        prev = Some(found);
        pos = found + 1;
    }
    Some(score)
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
    fn contiguous_and_boundary_matches_rank_higher() {
        assert!(score("web", "web-app") > score("web", "awesome-bed"));
        assert!(score("pooh", "pooh") > score("pooh", "pxoxoxoxh"));
    }
}

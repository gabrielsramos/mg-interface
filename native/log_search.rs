pub fn fuzzy_match(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    let mut chars = text.chars();
    query
        .to_lowercase()
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .all(|wanted| chars.by_ref().any(|ch| ch == wanted))
}

#[cfg(test)]
mod tests {
    use super::fuzzy_match;
    #[test]
    fn matches_case_insensitive_subsequences() {
        assert!(fuzzy_match("msdn", "Mouse moving down, diff: X=0, Y=7"));
        assert!(fuzzy_match("MOUSE DOWN", "Mouse moving down"));
        assert!(!fuzzy_match("down mouse", "Mouse moving down"));
        assert!(!fuzzy_match("error", "Mouse moving down"));
    }
    #[test]
    fn empty_query_restores_all_logs_and_unicode_is_supported() {
        assert!(fuzzy_match("  ", "anything"));
        assert!(fuzzy_match("", ""));
        assert!(fuzzy_match("OLÁ", "Olá mundo"));
        assert!(fuzzy_match("20:13", "20:13:47 Mouse moving down"));
    }
}

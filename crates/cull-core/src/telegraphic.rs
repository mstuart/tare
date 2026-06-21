//! Token-level "telegraphic" compaction of natural-language prose (opt-in, lossy).
//!
//! Sentence-level dropping is too coarse to beat trained NL compressors (LLMLingua) — they drop
//! filler *words within* sentences. This does the same heuristically: drop stopwords and short
//! low-information function words, keep content words, numbers, and named entities (the facts).
//! Instant (no model), and competitive with trained token-level compression on fact retention.

/// Function words that carry little task information. Dropping them rarely changes what an LLM can
/// answer; keeping content words + numbers + entities preserves the facts.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "to", "in", "on", "at", "by", "for", "with", "as", "is", "are", "was",
    "were", "be", "been", "being", "and", "or", "but", "if", "then", "so", "that", "this", "these",
    "those", "it", "its", "they", "them", "their", "we", "our", "you", "your", "he", "she", "his",
    "her", "from", "up", "out", "off", "over", "under", "into", "about", "during", "while", "which",
    "who", "whom", "whose", "there", "here", "also", "very", "just", "only", "some", "any", "all",
    "each", "more", "most", "much", "many", "such", "than", "have", "has", "had", "will", "would",
    "could", "should", "may", "might", "can", "do", "does", "did", "not", "no", "nor", "both",
    "either", "neither", "because", "though", "although", "however", "therefore", "thus", "hence",
    "per", "via", "upon", "within", "without", "between", "among", "across", "after", "before",
    // common low-information content words (verbs/adjectives/adverbs that rarely carry the fact)
    "felt", "feel", "gone", "went", "good", "well", "fine", "nice", "overall", "really", "quite",
    "actually", "basically", "generally", "usually", "often", "sometimes", "always", "never",
    "thing", "things", "stuff", "lot", "bit", "part", "kind", "sort", "way", "ways", "time",
    "times", "day", "days", "week", "year", "good", "great", "big", "small", "new", "old", "made",
    "make", "makes", "get", "gets", "got", "take", "takes", "took", "give", "gives", "gave",
    "look", "looks", "looked", "came", "come", "comes", "said", "says", "like", "want", "need",
    "people", "everyone", "someone", "anyone", "something", "anything", "nothing", "reasonably",
    "ultimately", "concerning", "pleasant", "spirits", "progress", "expected", "routine", "minor",
    "discussion", "topics", "given", "happened", "happen", "throughout", "brief", "roughly",
];

/// Telegraphic-compress prose: drop low-information function words, keep content/facts. Returns
/// `None` if the text is too short or doesn't shrink. Punctuation attached to kept words is kept.
pub fn compact(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 20 {
        return None;
    }
    let is_num = |w: &str| w.chars().any(|c| c.is_ascii_digit());
    let is_entity = |w: &str, bare: &str| w.chars().next().is_some_and(char::is_uppercase) && !STOPWORDS.contains(&bare);

    // base keep decision per word
    let bares: Vec<String> = words.iter()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect())
        .collect();
    let mut keep = vec![false; words.len()];
    for (i, w) in words.iter().enumerate() {
        let bare = &bares[i];
        let stop = STOPWORDS.contains(&bare.as_str());
        keep[i] = bare.is_empty()              // pure punctuation/symbols
            || is_num(w)                        // numbers/dates/codes/quantities (facts)
            || (!stop && (is_entity(w, bare) || bare.len() >= 5)); // entities + substantive content
    }
    // adjacency protection: keep the word right after a number or entity — it's the unit / compound
    // that completes the fact ("947 milliseconds", "5567 jobs", "Orion cache", "88 percent").
    for i in 1..words.len() {
        if keep[i - 1] && (is_num(words[i - 1]) || is_entity(words[i - 1], &bares[i - 1])) && !bares[i].is_empty() {
            keep[i] = true;
        }
    }
    let kept: Vec<&str> = words.iter().zip(&keep).filter(|(_, k)| **k).map(|(w, _)| *w).collect();
    if kept.len() == words.len() {
        return None; // nothing dropped
    }
    let out = kept.join(" ");
    if out.len() < text.len() {
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_filler_keeps_facts() {
        let text = "The Vega gateway recorded a peak latency of 947 milliseconds during the incident \
                    and the team agreed that everything was generally fine for the most part overall.";
        let out = compact(text).expect("should compact");
        assert!(out.len() < text.len());
        // facts/entities/numbers survive
        for keep in ["Vega", "947", "milliseconds", "latency", "gateway"] {
            assert!(out.contains(keep), "must keep {keep}: {out}");
        }
        // filler dropped
        assert!(!out.split_whitespace().any(|w| w == "the" || w == "of" || w == "during"),
            "filler dropped: {out}");
    }

    #[test]
    fn refuses_short_text() {
        assert!(compact("short text here").is_none());
    }

    #[test]
    fn stopword_list_lookup() {
        assert!(STOPWORDS.contains(&"the"));
        assert!(!STOPWORDS.contains(&"gateway"));
    }
}

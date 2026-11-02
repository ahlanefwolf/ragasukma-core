use std::collections::HashMap;

pub const MAX_SCORE: f64 = 1.0;

#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub file_path: &'a str,
    pub symbol_name: Option<&'a str>,
    pub summary: &'a str,
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(|w| w.to_lowercase())
        .collect()
}

fn document_frequencies(
    request_words: &[String],
    candidates: &[Candidate<'_>],
) -> HashMap<String, usize> {
    let candidate_words: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| {
            let mut words = tokenize(c.file_path);
            if let Some(sym) = c.symbol_name {
                words.extend(tokenize(sym));
            }
            words.extend(tokenize(c.summary));
            words
        })
        .collect();

    let mut frequencies: HashMap<String, usize> = HashMap::new();
    for word in request_words {
        if frequencies.contains_key(word) {
            continue;
        }
        let count = candidate_words
            .iter()
            .filter(|words| words.iter().any(|w| w == word))
            .count();
        frequencies.insert(word.clone(), count);
    }
    frequencies
}

pub fn score_candidate(
    request_words: &[String],
    candidates: &[Candidate<'_>],
    total_candidates: usize,
    file_index: usize,
) -> f64 {
    if request_words.is_empty() || total_candidates == 0 {
        return 0.0;
    }

    let unique_request_words: Vec<String> = {
        let mut seen: Vec<String> = Vec::with_capacity(request_words.len());
        for w in request_words {
            if !seen.contains(w) {
                seen.push(w.clone());
            }
        }
        seen
    };

    let frequencies = document_frequencies(&unique_request_words, candidates);
    let candidate = match candidates.get(file_index) {
        Some(c) => *c,
        None => return 0.0,
    };

    let mut file_words = tokenize(candidate.file_path);
    if let Some(sym) = candidate.symbol_name {
        file_words.extend(tokenize(sym));
    }
    file_words.extend(tokenize(candidate.summary));

    let total = total_candidates as f64;
    let mut matched_weight = 0.0;
    let mut available_weight = 0.0;

    for word in &unique_request_words {
        let frequency = frequencies.get(word).copied().unwrap_or(0);
        if frequency == 0 || frequency == total_candidates {
            continue;
        }
        let specificity = (total / frequency as f64).ln() + 1.0;
        available_weight += specificity;
        if file_words.iter().any(|w| w == word) {
            matched_weight += specificity;
        }
    }

    if available_weight <= 0.0 {
        return 0.0;
    }

    (matched_weight / available_weight).clamp(0.0, MAX_SCORE)
}

pub fn request_words(text: &str) -> Vec<String> {
    tokenize(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(paths: &[(&str, &str)]) -> Vec<Candidate<'static>> {
        paths
            .iter()
            .map(|(path, summary)| Candidate {
                file_path: Box::leak(path.to_string().into_boxed_str()),
                symbol_name: None,
                summary: Box::leak(summary.to_string().into_boxed_str()),
            })
            .collect()
    }

    #[test]
    fn a_long_request_does_not_dilute_the_score() {
        let cands = candidates(&[
            ("src/auth/login.rs", "handles login flow"),
            ("src/ui/button.rs", "renders a button"),
        ]);
        let request = request_words(
            "tolong perbaiki login yang error itu ya bro please fix the login thing because it keeps failing on me",
        );
        let login = score_candidate(&request, &cands, cands.len(), 0);
        let button = score_candidate(&request, &cands, cands.len(), 1);
        assert!(login > 0.8, "the login file answers the request, got {login}");
        assert_eq!(button, 0.0, "an unrelated file must score zero, got {button}");
    }

    #[test]
    fn a_word_shared_by_every_file_carries_no_weight() {
        let cands = candidates(&[
            ("src/parser.rs", "parse input"),
            ("src/writer.rs", "write output"),
        ]);
        let request = request_words("src writer");
        let writer = score_candidate(&request, &cands, cands.len(), 1);
        let parser = score_candidate(&request, &cands, cands.len(), 0);
        assert!(writer > parser, "the distinguishing word must decide: {writer} vs {parser}");
        assert_eq!(parser, 0.0);
    }

    #[test]
    fn repetition_inside_the_request_changes_nothing() {
        let cands = candidates(&[
            ("src/auth/login.rs", "login flow"),
            ("src/ui/button.rs", "renders a button"),
        ]);
        let once = score_candidate(&request_words("login"), &cands, cands.len(), 0);
        let thrice = score_candidate(&request_words("login login login"), &cands, cands.len(), 0);
        assert_eq!(once, thrice);
    }

    #[test]
    fn an_empty_request_scores_zero() {
        let cands = candidates(&[("src/auth/login.rs", "login flow")]);
        assert_eq!(score_candidate(&[], &cands, cands.len(), 0), 0.0);
        assert_eq!(score_candidate(&request_words("   "), &cands, cands.len(), 0), 0.0);
    }

    #[test]
    fn a_rare_word_outweighs_a_common_one() {
        let cands = candidates(&[
            ("src/shared.rs", "shared helper"),
            ("src/other.rs", "shared helper too"),
            ("src/specific.rs", "unique handler"),
        ]);
        let common_only = score_candidate(&request_words("shared"), &cands, cands.len(), 0);
        let rare_only = score_candidate(&request_words("unique"), &cands, cands.len(), 2);
        assert_eq!(common_only, 1.0);
        assert_eq!(rare_only, 1.0);

        let request = request_words("shared unique");
        let common_file = score_candidate(&request, &cands, cands.len(), 0);
        let rare_file = score_candidate(&request, &cands, cands.len(), 2);
        assert!(
            rare_file > common_file,
            "matching only the rare word must beat matching only the common word: {rare_file} vs {common_file}"
        );
    }
}
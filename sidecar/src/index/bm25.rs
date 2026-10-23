//! BM25 ranking over indexed source text. Pure functions + the ranker over
//! [`Index`]'s text corpus; kept apart from the storage type.

use super::Index;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

impl Index {
    /// Rank all indexed files by BM25 against `query_terms`. Returns
    /// `(path, score)` pairs, descending. Skips files that score zero.
    pub async fn bm25_rank(&self, query_terms: &[&str]) -> Vec<(PathBuf, f64)> {
        if query_terms.is_empty() {
            return Vec::new();
        }
        let texts = self.texts.read().await;
        let lens = self.doc_lens.read().await;
        let n_docs = texts.len();
        if n_docs == 0 {
            return Vec::new();
        }
        let avg_len = lens.values().sum::<usize>() as f64 / n_docs as f64;
        if avg_len == 0.0 {
            return Vec::new();
        }
        // Document frequency per term.
        let mut token_cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
        for (p, t) in texts.iter() {
            token_cache.insert(p.clone(), tokenize(t));
        }
        let mut df: HashMap<&str, usize> = HashMap::new();
        for toks in token_cache.values() {
            let mut seen = HashSet::new();
            for q in query_terms {
                if toks.iter().any(|t| t == q) && seen.insert(*q) {
                    *df.entry(*q).or_insert(0) += 1;
                }
            }
        }
        let mut out: Vec<(PathBuf, f64)> = Vec::new();
        for (p, toks) in token_cache.iter() {
            let len = lens.get(p).copied().unwrap_or(1);
            let s = bm25_score(query_terms, toks, len as f64, avg_len, n_docs, &df);
            if s > 0.0 {
                out.push((p.clone(), s));
            }
        }
        out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        out
    }
}

/// Split `text` into lowercase identifier-ish tokens. Splits snake_case,
/// SCREAMING_CASE, and camelCase so "getUserName" matches "get", "user", "name"
/// and "USER_ID" matches "user", "id".
pub fn tokenize(text: &str) -> Vec<String> {
    fn commit(cur: &mut String, out: &mut Vec<String>) {
        if cur.is_empty() {
            return;
        }
        // snake_case is split at commit time so "FOO_BAR" gives "foo","bar".
        for piece in cur.split('_') {
            if !piece.is_empty() {
                out.push(piece.to_string());
            }
        }
        cur.clear();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower_or_digit = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            let camel_break = ch.is_uppercase() && prev_lower_or_digit;
            if camel_break {
                commit(&mut cur, &mut out);
            }
            cur.push(ch.to_lowercase().next().unwrap_or(ch));
            prev_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        } else {
            commit(&mut cur, &mut out);
            prev_lower_or_digit = false;
        }
    }
    commit(&mut cur, &mut out);
    out
}

/// Standard BM25 (k1=1.5, b=0.75). `doc_tokens` is the pre-tokenized document.
/// ponytail: O(terms × tokens) scan; fine while ranked docs are hundreds of lines.
pub fn bm25_score(
    query_terms: &[&str],
    doc_tokens: &[String],
    doc_len: f64,
    avg_len: f64,
    n_docs: usize,
    df: &HashMap<&str, usize>,
) -> f64 {
    const K1: f64 = 1.5;
    const B: f64 = 0.75;
    if doc_tokens.is_empty() {
        return 0.0;
    }
    // Term freq per query term.
    let mut tf: HashMap<&str, usize> = HashMap::new();
    for t in doc_tokens {
        for q in query_terms {
            if t == q {
                *tf.entry(*q).or_insert(0) += 1;
            }
        }
    }
    let mut score = 0.0;
    for (q, f) in tf.iter() {
        let d = df.get(*q).copied().unwrap_or(0);
        if d == 0 {
            continue;
        }
        // Robertson IDF, clamped at zero for very common terms.
        let idf = ((n_docs as f64 - d as f64 + 0.5) / (d as f64 + 0.5) + 1.0).ln();
        let tf_norm =
            (*f as f64) * (K1 + 1.0) / (*f as f64 + K1 * (1.0 - B + B * doc_len / avg_len));
        score += idf * tf_norm;
    }
    score
}

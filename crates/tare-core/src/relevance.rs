//! Opt-in semantic relevance ranking via embeddings.
//!
//! # Default behavior (unchanged)
//!
//! The keyword/symbol BFS pass in `passes/relevance.rs` remains the default relevance
//! mechanism. This module adds NO always-on overhead, NO model download, and NO new
//! mandatory dependencies to the default build.
//!
//! # Opt-in semantic ranking
//!
//! Callers that want embedding-based ranking can call [`rank_by_relevance`] directly.
//! It is generic over [`Embedder`], so it works with the always-compiled [`StubEmbedder`]
//! (deterministic, dependency-free) or, behind the `neural-embed` feature, the
//! [`crate::embed_neural::FastEmbedder`].
//!
//! Example of opt-in use alongside the existing pass:
//! ```ignore
//! use tare_core::relevance::{StubEmbedder, rank_by_relevance};
//! let ranked = rank_by_relevance("jwt auth", &["token expiry check", "kafka broker"], &StubEmbedder::default());
//! ```

use xxhash_rust::xxh3::xxh3_64;

// ─── Trait ────────────────────────────────────────────────────────────────────

/// Batch text embedder. Returns one vector per input text; all vectors have the
/// same dimension (consistent within an impl). Dimension must be > 0.
///
/// This trait is **always compiled** (not feature-gated).  The [`StubEmbedder`]
/// provides a deterministic, dependency-free implementation for default builds and
/// unit tests.  The `neural-embed` feature enables [`crate::embed_neural::FastEmbedder`]
/// as a drop-in that satisfies this trait with real ONNX embeddings.
pub trait Embedder {
    fn embed(&self, texts: &[&str]) -> Vec<Vec<f32>>;
}

// ─── StubEmbedder ─────────────────────────────────────────────────────────────

/// Deterministic, dependency-free stub embedder.
///
/// Maps each text to a fixed-dim L2-normalized vector by hashing word unigrams and
/// character bigrams into a bag-of-features array via xxh3.  No RNG; fully
/// deterministic.  Useful for unit tests and as a cheap lexical-semantic fallback
/// in environments where a neural model is unavailable.
pub struct StubEmbedder {
    dim: usize,
}

impl StubEmbedder {
    pub fn new(dim: usize) -> Self {
        Self { dim: dim.max(1) }
    }
}

impl Default for StubEmbedder {
    fn default() -> Self {
        Self::new(256)
    }
}

impl Embedder for StubEmbedder {
    fn embed(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        texts.iter().map(|t| embed_stub(t, self.dim)).collect()
    }
}

/// Hash word unigrams and character bigrams into a bag-of-features vector, then
/// L2-normalize.  Two texts sharing words or sub-word sequences will land in
/// overlapping buckets, producing a higher cosine score.
fn embed_stub(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0f32; dim];
    let lower = text.to_ascii_lowercase();
    for tok in lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        // word unigram
        let h = (xxh3_64(tok.as_bytes()) as usize) % dim;
        v[h] += 1.0;
        // character bigrams (sub-word signal)
        for w in tok.as_bytes().windows(2) {
            let h2 = (xxh3_64(w) as usize) % dim;
            v[h2] += 0.5;
        }
    }
    l2_normalize(&mut v);
    v
}

// ─── Exact cosine (in-house, no HNSW / vector crate) ─────────────────────────

/// Full cosine similarity: dot(a, b) / (|a| * |b|).  Returns 0.0 for zero vectors
/// or dimension mismatch.  Works with both normalized and unnormalized vectors.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

// ─── Public ranking API ───────────────────────────────────────────────────────

/// Rank `candidates` by cosine similarity to `query` using the given embedder.
///
/// Returns `(original_index, score)` pairs sorted descending (most similar first).
/// Empty `candidates` is safe and returns an empty vec.
///
/// # Opt-in note
///
/// This function is **opt-in**.  The default relevance pass (`passes/relevance.rs`)
/// uses keyword/symbol BFS and is completely unaffected.  Call this function only
/// when you want embedding-based ranking in addition to — or instead of — the
/// keyword pass.
///
/// At relevance-pass scale (tens to low hundreds of candidates) exact cosine beats
/// approximate nearest-neighbour (HNSW / vector-crate) and avoids a heavy
/// dependency.
pub fn rank_by_relevance<E: Embedder>(
    query: &str,
    candidates: &[&str],
    embedder: &E,
) -> Vec<(usize, f32)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    // Embed query + all candidates in a single batch call.
    let mut all: Vec<&str> = Vec::with_capacity(candidates.len() + 1);
    all.push(query);
    all.extend_from_slice(candidates);
    let mut vecs = embedder.embed(&all);
    // First vector is the query; the rest are candidates.
    let query_vec = vecs.remove(0);
    let mut scored: Vec<(usize, f32)> = vecs
        .iter()
        .enumerate()
        .map(|(i, v)| (i, cosine(&query_vec, v)))
        .collect();
    scored.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

// ─── Tests (default features; StubEmbedder only) ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_ranks_related_above_unrelated() {
        // A query-relevant candidate must score higher than an unrelated one.
        let e = StubEmbedder::default();
        let ranked = rank_by_relevance(
            "jwt authentication token",
            &[
                "oauth bearer token expiry", // shares "token" — should rank first
                "kubernetes helm chart registry docker",
            ],
            &e,
        );
        assert_eq!(ranked.len(), 2);
        assert_eq!(
            ranked[0].0, 0,
            "related candidate (index 0) must rank first; got index {}",
            ranked[0].0
        );
        assert!(
            ranked[0].1 > ranked[1].1,
            "related score {:.4} must exceed unrelated score {:.4}",
            ranked[0].1,
            ranked[1].1
        );
    }

    #[test]
    fn stub_embedder_is_deterministic() {
        let e = StubEmbedder::new(128);
        let a = rank_by_relevance("hello world", &["foo bar", "baz qux rust"], &e);
        let b = rank_by_relevance("hello world", &["foo bar", "baz qux rust"], &e);
        assert_eq!(a, b, "rank_by_relevance must be deterministic");
    }

    #[test]
    fn rank_by_relevance_empty_candidates_is_safe() {
        let e = StubEmbedder::default();
        let result = rank_by_relevance("some query", &[], &e);
        assert!(result.is_empty(), "empty candidates must return empty vec");
    }

    #[test]
    fn stub_single_candidate_returns_one_entry() {
        let e = StubEmbedder::default();
        let result = rank_by_relevance("query text", &["only candidate"], &e);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 0);
    }

    #[test]
    fn cosine_zero_vector_returns_zero() {
        // embed("") → zero vector after normalization guard
        let e = StubEmbedder::default();
        let vecs = e.embed(&[""]);
        // All-zero vector: cosine must not panic and return 0.0
        let result = cosine(&vecs[0], &vecs[0]);
        assert_eq!(result, 0.0);
    }
}

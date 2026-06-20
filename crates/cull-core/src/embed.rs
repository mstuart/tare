use xxhash_rust::xxh3::xxh3_64;

/// Maps text to a fixed-dimension vector for salience scoring (spec §7 B3).
pub trait Embedder {
    fn embed(&self, text: &str) -> Vec<f32>;
    fn dim(&self) -> usize;
}

/// Dependency-free hashing embedder (the hashing trick): lowercased alphanumeric tokens are hashed
/// into `dim` buckets as an L2-normalized term-frequency vector. Cosine similarity over these
/// vectors is a lexical/semantic salience signal complementary to symbol extraction (B1). A neural
/// embedder can replace it behind the `Embedder` trait without changing the pass.
pub struct HashEmbedder { dim: usize }

impl HashEmbedder {
    pub fn new(dim: usize) -> Self { Self { dim: dim.max(1) } }
}
impl Default for HashEmbedder { fn default() -> Self { Self::new(256) } }

impl Embedder for HashEmbedder {
    fn dim(&self) -> usize { self.dim }
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut v = vec![0f32; self.dim];
        for tok in text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
            let h = (xxh3_64(tok.to_ascii_lowercase().as_bytes()) as usize) % self.dim;
            v[h] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 { for x in &mut v { *x /= norm; } }
        v
    }
}

/// Cosine similarity. For the L2-normalized vectors `HashEmbedder` produces this is the dot
/// product. Returns 0.0 on length mismatch or a zero vector.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_text_scores_higher_than_unrelated() {
        let e = HashEmbedder::default();
        let q = e.embed("jwt authentication token expiry");
        let related = e.embed("token authentication jwt expired");
        let unrelated = e.embed("kubernetes helm chart registry docker");
        assert!(cosine(&q, &related) > cosine(&q, &unrelated));
        assert!(cosine(&q, &related) > 0.5, "related sim {}", cosine(&q, &related));
        assert!(cosine(&q, &unrelated) < 0.1, "unrelated sim {}", cosine(&q, &unrelated));
    }

    #[test]
    fn embedding_is_deterministic_and_normalized() {
        let e = HashEmbedder::new(128);
        assert_eq!(e.embed("hello world"), e.embed("hello world"));
        let norm: f32 = e.embed("hello world").iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "L2 norm {norm}");
    }

    #[test]
    fn cosine_handles_mismatched_lengths_and_empty() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0]), 0.0);
        assert_eq!(cosine(&HashEmbedder::default().embed(""), &HashEmbedder::default().embed("x")), 0.0);
    }
}

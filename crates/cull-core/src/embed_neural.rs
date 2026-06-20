//! Neural embedding backend via fastembed (ONNX).
//! Compiled only when the `neural-embed` feature is enabled.

use std::sync::Mutex;

use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use crate::embed::Embedder;

/// Neural embedder backed by a local ONNX model (fastembed).
///
/// On construction the model weights are downloaded from HuggingFace Hub on
/// first use and cached in `~/.cache/huggingface`. Subsequent calls are
/// offline. The model is wrapped in a `Mutex` so that the `Embedder` trait's
/// `&self` embed signature is satisfied even though fastembed's internal
/// `TextEmbedding::embed` requires `&mut self`.
pub struct FastEmbedder {
    model: Mutex<TextEmbedding>,
    dim: usize,
}

impl FastEmbedder {
    /// Construct a `FastEmbedder` using `BGESmallENV15` (384-dim, ~33 MB).
    /// Downloads the model if not already cached.
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15),
        )?;
        Ok(Self { model: Mutex::new(model), dim: 384 })
    }
}

impl Embedder for FastEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut guard = self.model.lock().expect("fastembed mutex poisoned");
        match guard.embed(vec![text], None) {
            Ok(mut vecs) if !vecs.is_empty() => vecs.remove(0),
            _ => vec![0.0f32; self.dim],
        }
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::cosine;

    /// Verifies semantic similarity ordering: "jwt authentication token" should
    /// be closer to another auth-related sentence than to a k8s sentence.
    ///
    /// Marked `#[ignore]` because construction downloads the model (~33 MB) and
    /// is unsuitable for offline / CI environments. Run explicitly with:
    ///   cargo test -p cull-core --features neural-embed -- --ignored embed_neural
    #[test]
    #[ignore]
    fn semantic_similarity_ordering() {
        let embedder = FastEmbedder::new().expect("model init failed");
        let query = embedder.embed("jwt authentication token");
        let related = embedder.embed("oauth bearer token expiry");
        let unrelated = embedder.embed("kubernetes helm chart registry docker");
        let sim_related = cosine(&query, &related);
        let sim_unrelated = cosine(&query, &unrelated);
        assert!(
            sim_related > sim_unrelated,
            "expected cosine(query, related)={sim_related:.4} > cosine(query, unrelated)={sim_unrelated:.4}"
        );
    }
}

use std::collections::HashMap;
use xxhash_rust::xxh3::xxh3_64;
use crate::segment::{RefLedger, SegmentId};

#[derive(Debug, Clone)]
pub struct CanonicalFile { pub bytes: Vec<u8>, pub token_count: u32, pub version: u32 }

/// File-read IVM baseline store (spec A2): path -> canonical snapshot.
#[derive(Debug, Default)]
pub struct CanonicalFileStore { map: HashMap<String, CanonicalFile> }

impl CanonicalFileStore {
    pub fn get(&self, path: &str) -> Option<&CanonicalFile> { self.map.get(path) }
    pub fn put(&mut self, path: &str, bytes: Vec<u8>, token_count: u32) {
        let version = self.map.get(path).map(|f| f.version + 1).unwrap_or(0);
        self.map.insert(path.to_string(), CanonicalFile { bytes, token_count, version });
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ToolRun { pub turn: usize, pub exit_code: Option<i32> }

/// Supersession registry (spec A1): tool class -> latest run.
#[derive(Debug, Default)]
pub struct ToolClassRegistry { latest: HashMap<String, ToolRun> }

impl ToolClassRegistry {
    pub fn record(&mut self, class: &str, turn: usize, exit_code: Option<i32>) {
        self.latest.insert(class.to_string(), ToolRun { turn, exit_code });
    }
    pub fn latest_run(&self, class: &str) -> Option<ToolRun> { self.latest.get(class).copied() }
}

/// Span reference/recency/phase ledger (spec C1/C2 eviction inputs).
#[derive(Debug, Default)]
pub struct SpanLedger { pub entries: HashMap<SegmentId, RefLedger> }

/// Cache-prefix commitment (spec Rule 1/7): the frozen zone is a content hash.
#[derive(Debug, Default)]
pub struct CachePrefixCommitment { pub frozen_hash: Option<u64>, pub frozen_len_tokens: usize }

impl CachePrefixCommitment {
    pub fn commit(&mut self, frozen_bytes: &[u8], frozen_len_tokens: usize) {
        self.frozen_hash = Some(xxh3_64(frozen_bytes));
        self.frozen_len_tokens = frozen_len_tokens;
    }
}

/// Aggregate per-session engine state (passes in later plans read/write these).
#[derive(Debug, Default)]
pub struct SessionState {
    pub files: CanonicalFileStore,
    pub tools: ToolClassRegistry,
    pub spans: SpanLedger,
    pub prefix: CachePrefixCommitment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_store_inserts_and_reads() {
        let mut store = CanonicalFileStore::default();
        store.put("src/a.rs", b"contents".to_vec(), 2);
        let f = store.get("src/a.rs").unwrap();
        assert_eq!(f.version, 0);
        assert_eq!(f.token_count, 2);
        store.put("src/a.rs", b"new".to_vec(), 1);
        assert_eq!(store.get("src/a.rs").unwrap().version, 1);
    }

    #[test]
    fn prefix_commitment_is_stable_for_same_bytes() {
        let mut c = CachePrefixCommitment::default();
        c.commit(b"frozen-prefix-bytes", 5);
        let h1 = c.frozen_hash;
        c.commit(b"frozen-prefix-bytes", 5);
        assert_eq!(h1, c.frozen_hash);
        c.commit(b"different", 2);
        assert_ne!(h1, c.frozen_hash);
    }

    #[test]
    fn tool_registry_tracks_latest_run() {
        let mut r = ToolClassRegistry::default();
        r.record("cargo-test", 12, Some(1));
        r.record("cargo-test", 31, Some(0));
        let run = r.latest_run("cargo-test").unwrap();
        assert_eq!(run.turn, 31);
        assert_eq!(run.exit_code, Some(0));
    }
}

//! Persistent cross-agent memory: SQLite-backed remember/recall with content-hash dedup
//! and multi-source provenance tracking.

use rusqlite::{params, Connection, Result};
use xxhash_rust::xxh3::xxh3_64;

/// A single memory hit returned by [`Memory::recall`].
#[derive(Debug, Clone)]
pub struct Match {
    pub id: i64,
    pub content: String,
    /// Comma-joined list of all recorded sources for this memory.
    pub source: String,
    /// Score ≥ 0.0 — number of query terms found in content (0.0 for empty-query results).
    pub score: f64,
}

/// Aggregate statistics for the store.
#[derive(Debug, Clone)]
pub struct MemStats {
    pub count: i64,
    pub sources: usize,
}

/// SQLite-backed persistent memory store.
pub struct Memory {
    conn: Connection,
}

impl Memory {
    /// Open (or create) a memory store at `path`.  Use `":memory:"` for an
    /// in-process ephemeral store.  The schema is created on first open.
    pub fn open(path: &str) -> Result<Memory> {
        let conn = Connection::open(path)?;
        // WAL gives better concurrent-read throughput; silently ignored for :memory:.
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                content      TEXT    NOT NULL,
                content_hash TEXT    NOT NULL UNIQUE,
                created_at   TEXT    NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS provenance (
                memory_id INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                source    TEXT    NOT NULL,
                UNIQUE(memory_id, source)
            );",
        )?;
        Ok(Memory { conn })
    }

    /// Open the default store.
    ///
    /// Resolution order:
    /// 1. `$TARE_MEMORY`
    /// 2. `$XDG_CONFIG_HOME/tare/memory.db`
    /// 3. `~/.config/tare/memory.db`
    ///
    /// The parent directory is created with `mkdir -p` semantics if absent.
    pub fn open_default() -> Result<Memory> {
        let path = if let Ok(p) = std::env::var("TARE_MEMORY") {
            std::path::PathBuf::from(p)
        } else {
            let base = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
                std::path::PathBuf::from(xdg)
            } else {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                std::path::PathBuf::from(home).join(".config")
            };
            base.join("tare").join("memory.db")
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| rusqlite::Error::InvalidPath(e.to_string().into()))?;
        }

        Memory::open(path.to_str().unwrap_or("memory.db"))
    }

    /// Persist a memory.
    ///
    /// Dedup is by content hash (xxh3-64).  If `content` already exists the
    /// new `source` is appended to its provenance and the **existing** id is
    /// returned unchanged.  Otherwise a new row is inserted.
    pub fn remember(&self, content: &str, source: &str) -> Result<i64> {
        let hash = format!("{:016x}", xxh3_64(content.as_bytes()));

        // INSERT OR IGNORE — no-op if content_hash already exists.
        self.conn.execute(
            "INSERT OR IGNORE INTO memories (content, content_hash) VALUES (?1, ?2)",
            params![content, hash],
        )?;

        // Fetch the id regardless of whether we just inserted or it pre-existed.
        let id: i64 = self.conn.query_row(
            "SELECT id FROM memories WHERE content_hash = ?1",
            params![hash],
            |row| row.get(0),
        )?;

        // Record provenance; UNIQUE(memory_id, source) prevents duplicates.
        self.conn.execute(
            "INSERT OR IGNORE INTO provenance (memory_id, source) VALUES (?1, ?2)",
            params![id, source],
        )?;

        Ok(id)
    }

    /// Search memories.
    ///
    /// - **Non-empty query**: splits into whitespace-delimited terms, counts
    ///   case-insensitive substring hits per memory, returns matches (score > 0)
    ///   ordered by hit-count descending, truncated to `limit`.
    /// - **Empty query**: returns the `limit` most-recently-created memories
    ///   (score = 0.0).
    pub fn recall(&self, query: &str, limit: usize) -> Result<Vec<Match>> {
        let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();

        // Fetch all memories with comma-joined sources.
        let mut stmt = self.conn.prepare(
            "SELECT m.id, m.content, COALESCE(GROUP_CONCAT(p.source, ', '), '') AS sources
             FROM memories m
             LEFT JOIN provenance p ON p.memory_id = m.id
             GROUP BY m.id
             ORDER BY m.id DESC",
        )?;

        let rows: Vec<(i64, String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<_>>()?;

        if terms.is_empty() {
            // Empty query → most recent N, score 0.0.
            return Ok(rows
                .into_iter()
                .take(limit)
                .map(|(id, content, source)| Match {
                    id,
                    content,
                    source,
                    score: 0.0,
                })
                .collect());
        }

        // Score by number of matching terms.
        let mut matches: Vec<Match> = rows
            .into_iter()
            .filter_map(|(id, content, source)| {
                let lower = content.to_lowercase();
                let score = terms.iter().filter(|t| lower.contains(t.as_str())).count() as f64;
                if score > 0.0 {
                    Some(Match {
                        id,
                        content,
                        source,
                        score,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Stable sort: higher score first.
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches.truncate(limit);
        Ok(matches)
    }

    /// Delete a memory by id (cascades to provenance).
    ///
    /// Returns `true` if a row was deleted, `false` if `id` was not found.
    pub fn forget(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM memories WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Return aggregate store statistics.
    pub fn stats(&self) -> Result<MemStats> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        let sources: i64 =
            self.conn
                .query_row("SELECT COUNT(DISTINCT source) FROM provenance", [], |row| {
                    row.get(0)
                })?;
        Ok(MemStats {
            count,
            sources: sources as usize,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Memory {
        Memory::open(":memory:").expect("in-memory db")
    }

    #[test]
    fn remember_and_recall_round_trip() {
        let m = mem();
        let id = m.remember("Rust is fast and safe", "agent-a").unwrap();
        let hits = m.recall("rust fast", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, id);
        assert!(hits[0].content.contains("Rust"));
    }

    #[test]
    fn dedup_returns_same_id_and_records_both_sources() {
        let m = mem();
        let id1 = m.remember("shared knowledge", "agent-a").unwrap();
        let id2 = m.remember("shared knowledge", "agent-b").unwrap();
        assert_eq!(id1, id2, "identical content must return the same id");

        // Both sources should appear in recall results.
        let hits = m.recall("shared knowledge", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0].source.contains("agent-a"),
            "source should include agent-a"
        );
        assert!(
            hits[0].source.contains("agent-b"),
            "source should include agent-b"
        );
    }

    #[test]
    fn recall_ranks_better_match_first() {
        let m = mem();
        // "alpha beta gamma" matches 3 of 3 terms; "alpha only" matches 1 of 3.
        m.remember("alpha only content", "s").unwrap();
        m.remember("alpha beta gamma content", "s").unwrap();

        let hits = m.recall("alpha beta gamma", 10).unwrap();
        assert!(hits.len() >= 2);
        assert!(
            hits[0].score >= hits[1].score,
            "better match must come first"
        );
        assert!(
            hits[0].content.contains("gamma"),
            "top hit should be the 3-term match"
        );
    }

    #[test]
    fn forget_removes_memory() {
        let m = mem();
        let id = m.remember("to be forgotten", "s").unwrap();
        assert!(m.forget(id).unwrap(), "should return true when row existed");
        let hits = m.recall("forgotten", 10).unwrap();
        assert!(hits.is_empty());
        assert!(
            !m.forget(id).unwrap(),
            "should return false when already gone"
        );
    }

    #[test]
    fn stats_counts_correctly() {
        let m = mem();
        m.remember("one", "a").unwrap();
        m.remember("two", "a").unwrap();
        m.remember("three", "b").unwrap();
        let s = m.stats().unwrap();
        assert_eq!(s.count, 3);
        assert_eq!(s.sources, 2); // distinct: "a", "b"
    }

    #[test]
    fn empty_query_returns_most_recent() {
        let m = mem();
        m.remember("first", "s").unwrap();
        m.remember("second", "s").unwrap();
        m.remember("third", "s").unwrap();
        let hits = m.recall("", 2).unwrap();
        assert_eq!(hits.len(), 2);
        // Most recent first.
        assert_eq!(hits[0].content, "third");
        assert_eq!(hits[1].content, "second");
    }
}

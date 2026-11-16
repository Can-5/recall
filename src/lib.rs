//! Recall: local-first clipboard / text history domain logic.
//!
//! Recall runs on the user's machine, never phones home, and has no opinion about
//! *how* entries are captured from the OS — the Orbit host app is responsible for
//! clipboard monitoring (and only after the user grants the Permission). This crate
//! owns the data model, retention policy, search and exclusion rules.
//!
//! Retention: the history is capped at `capacity` entries (default 500, max 10_000).
//! Pinned/favorite entries are **never** evicted by the cap. All matching is plain
//! text — no regex is accepted from untrusted input.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;

pub const DEFAULT_CAPACITY: usize = 500;
pub const MAX_CAPACITY: usize = 10_000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RecallError {
    #[error("capacity {0} exceeds maximum of {MAX_CAPACITY}")]
    CapacityTooLarge(usize),
    #[error("capacity must be at least 1")]
    CapacityZero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Text,
    Image,
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub kind: EntryKind,
    /// Preview / full text. For images this is a placeholder reference, not pixels.
    pub text: String,
    /// Optional note the user attached.
    pub note: Option<String>,
    pub favorite: bool,
    pub created_at_ms: u64,
    /// Source application id, if the host was allowed to see it.
    pub source_app: Option<String>,
}

impl HistoryEntry {
    pub fn new(id: impl Into<String>, kind: EntryKind, text: impl Into<String>, created_at_ms: u64) -> Self {
        Self {
            id: id.into(),
            kind,
            text: text.into(),
            note: None,
            favorite: false,
            created_at_ms,
            source_app: None,
        }
    }
}

/// Plain-text exclusion rules: entries whose text or source app contains any of the
/// denylist substrings (case-insensitive) are refused at push time. Deliberately no
/// regex — denylist items are literal secrets material users paste (API keys, cards).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExclusionRules {
    blocked_substrings: Vec<String>,
}

impl ExclusionRules {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn block(&mut self, needle: impl Into<String>) {
        let n = needle.into();
        let n = n.trim().to_lowercase();
        if !n.is_empty() && !self.blocked_substrings.contains(&n) {
            self.blocked_substrings.push(n);
        }
    }

    pub fn unblock(&mut self, needle: &str) {
        let n = needle.trim().to_lowercase();
        self.blocked_substrings.retain(|s| s != &n);
    }

    pub fn is_blocked(&self, entry: &HistoryEntry) -> bool {
        let hay_text = entry.text.to_lowercase();
        let hay_app = entry.source_app.as_deref().unwrap_or("").to_lowercase();
        self.blocked_substrings
            .iter()
            .any(|n| hay_text.contains(n) || hay_app.contains(n))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallHistory {
    capacity: usize,
    entries: VecDeque<HistoryEntry>,
    pub rules: ExclusionRules,
}

impl RecallHistory {
    pub fn with_capacity(capacity: usize) -> Result<Self, RecallError> {
        if capacity == 0 {
            return Err(RecallError::CapacityZero);
        }
        if capacity > MAX_CAPACITY {
            return Err(RecallError::CapacityTooLarge(capacity));
        }
        Ok(Self { capacity, entries: VecDeque::with_capacity(capacity), rules: ExclusionRules::new() })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Push an entry; returns false (and stores nothing) when it matches the exclusion rules.
    /// Eviction policy: oldest *non-favorite* entry is dropped when full; favorites are sticky.
    pub fn push(&mut self, entry: HistoryEntry) -> bool {
        if self.rules.is_blocked(&entry) {
            return false;
        }
        self.entries.push_back(entry);
        self.evict_if_needed();
        true
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.capacity {
            // Find oldest non-favorite; if all are favorites, stop evicting (history may exceed cap).
            if let Some(idx) = self.entries.iter().position(|e| !e.favorite) {
                self.entries.remove(idx);
            } else {
                break;
            }
        }
    }

    pub fn toggle_favorite(&mut self, id: &str) -> bool {
        if let Some(e) = self.entries.iter_mut().find(|e| e.id == id) {
            e.favorite = !e.favorite;
            return true;
        }
        false
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn clear_favorites(&mut self) {
        self.entries.retain(|e| e.favorite);
    }

    /// Case-insensitive substring search over text, note and source app. Newest first.
    pub fn search(&self, query: &str) -> Vec<&HistoryEntry> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return self.entries.iter().rev().collect();
        }
        self.entries
            .iter()
            .rev()
            .filter(|e| {
                e.text.to_lowercase().contains(&q)
                    || e.note.as_deref().unwrap_or("").to_lowercase().contains(&q)
                    || e.source_app.as_deref().unwrap_or("").to_lowercase().contains(&q)
            })
            .collect()
    }

    /// Most recent entries, newest first, capped at `limit`.
    pub fn recent(&self, limit: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(limit).collect()
    }

    /// Remove *consecutive* entries that share kind and payload (`text`).
    /// The first entry of each run is kept. Returns how many were removed.
    pub fn dedupe_consecutive(&mut self) -> usize {
        let mut removed = 0usize;
        let mut i = 1;
        while i < self.entries.len() {
            let (prev, cur) = (&self.entries[i - 1], &self.entries[i]);
            if prev.kind == cur.kind && prev.text == cur.text {
                self.entries.remove(i);
                removed += 1;
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Point-in-time totals over the current history.
    pub fn stats(&self) -> Stats {
        let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut favorites = 0usize;
        for e in &self.entries {
            let kind = match e.kind {
                EntryKind::Text => "text",
                EntryKind::Image => "image",
                EntryKind::Link => "link",
            };
            *by_kind.entry(kind).or_default() += 1;
            if e.favorite {
                favorites += 1;
            }
        }
        Stats { total: self.entries.len(), favorites, by_kind }
    }
}

/// Aggregate counts over a [`RecallHistory`]. `by_kind` keys: "text" | "image" | "link".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stats {
    pub total: usize,
    pub favorites: usize,
    pub by_kind: BTreeMap<&'static str, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, text: &str, ts: u64) -> HistoryEntry {
        HistoryEntry::new(id, EntryKind::Text, text, ts)
    }

    #[test]
    fn capacity_is_enforced_oldest_first() {
        let mut h = RecallHistory::with_capacity(3).unwrap();
        for i in 0..5 {
            assert!(h.push(entry(&format!("e{i}"), &format!("item {i}"), i)));
        }
        assert_eq!(h.len(), 3);
        let ids: Vec<_> = h.recent(10).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["e4", "e3", "e2"]);
    }

    #[test]
    fn favorites_survive_eviction() {
        let mut h = RecallHistory::with_capacity(2).unwrap();
        h.push(entry("keep", "pinned", 1));
        h.toggle_favorite("keep");
        h.push(entry("a", "x", 2));
        h.push(entry("b", "y", 3)); // forces eviction of oldest non-fav ("a")
        let ids: Vec<_> = h.recent(10).iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"keep"));
        assert!(!ids.contains(&"a"));
    }

    #[test]
    fn exclusion_rules_block_case_insensitively() {
        let mut h = RecallHistory::with_capacity(10).unwrap();
        h.rules.block("API-KEY-123");
        assert!(!h.push(entry("s1", "my api-key-123 secret", 1)));
        assert_eq!(h.len(), 0);
        assert!(h.push(entry("ok", "hello world", 2)));
    }

    #[test]
    fn exclusion_rules_match_source_app() {
        let mut h = RecallHistory::with_capacity(10).unwrap();
        h.rules.block("bitwarden");
        let mut e = entry("pw", "hunter2", 1);
        e.source_app = Some("Bitwarden".into());
        assert!(!h.push(e));
    }

    #[test]
    fn search_is_case_insensitive_and_newest_first() {
        let mut h = RecallHistory::with_capacity(10).unwrap();
        h.push(entry("1", "Hello World", 1));
        h.push(entry("2", "hello again", 2));
        h.push(entry("3", "unrelated", 3));
        let hits: Vec<_> = h.search("HELLO").iter().map(|e| e.id.as_str()).collect();
        assert_eq!(hits, vec!["2", "1"]);
    }

    #[test]
    fn dedupe_consecutive_removes_only_runs() {
        let mut h = RecallHistory::with_capacity(10).unwrap();
        h.push(entry("1", "same", 1));
        h.push(entry("2", "same", 2));
        h.push(entry("3", "other", 3));
        h.push(entry("4", "same", 4)); // not consecutive to 1/2 — kept
        h.push(entry("5", "same", 5));
        let mut img = entry("6", "same", 6);
        img.kind = EntryKind::Image;
        h.push(img); // same text, different kind — kept
        assert_eq!(h.dedupe_consecutive(), 2); // ids 2 and 5 removed
        let ids: Vec<_> = h.recent(10).iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["6", "4", "3", "1"]);
        assert_eq!(h.dedupe_consecutive(), 0);
    }

    #[test]
    fn stats_totals_and_by_kind() {
        let mut h = RecallHistory::with_capacity(10).unwrap();
        h.push(entry("1", "a", 1));
        h.push(entry("2", "b", 2));
        let mut img = entry("3", "c", 3);
        img.kind = EntryKind::Image;
        h.push(img);
        let mut link = entry("4", "d", 4);
        link.kind = EntryKind::Link;
        h.push(link);
        h.toggle_favorite("2");
        let s = h.stats();
        assert_eq!(s.total, 4);
        assert_eq!(s.favorites, 1);
        assert_eq!(s.by_kind.get("text"), Some(&2));
        assert_eq!(s.by_kind.get("image"), Some(&1));
        assert_eq!(s.by_kind.get("link"), Some(&1));
        h.clear();
        let empty = h.stats();
        assert_eq!(empty.total, 0);
        assert!(empty.by_kind.is_empty());
    }

    #[test]
    fn invalid_capacity_rejected() {
        assert_eq!(RecallHistory::with_capacity(0).unwrap_err(), RecallError::CapacityZero);
        assert!(matches!(
            RecallHistory::with_capacity(MAX_CAPACITY + 1).unwrap_err(),
            RecallError::CapacityTooLarge(_)
        ));
    }
}

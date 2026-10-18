//! Shared fuzzy matching and frecency (frequency + exponential half-life recency) scoring
//! used across `chickenwm-launcher`, `chickenwm-files`, and `henbrowser`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Current Unix timestamp in seconds.
pub fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fuzzy score: every non-whitespace character of `query` must appear in order in `hay`.
/// Awards bonuses for contiguous runs and word-start boundaries (`\`, `/`, ` `, `-`, `_`, `.`).
/// Returns `None` if `query` does not match `hay`.
pub fn fuzzy_score(query: &str, hay: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    let mut score = 0i64;
    let mut hi = 0usize;
    let mut prev: Option<usize> = None;
    for qc in query.to_lowercase().chars().filter(|c| !c.is_whitespace()) {
        let pos = (hi..hay.len()).find(|&i| hay[i] == qc)?;
        score += 10;
        if prev == Some(pos.wrapping_sub(1)) {
            score += 15;
        }
        if pos == 0 || matches!(hay[pos - 1], '\\' | '/' | ' ' | '-' | '_' | '.') {
            score += 20;
        }
        prev = Some(pos);
        hi = pos + 1;
    }
    Some(score - hay.len() as i64 / 4)
}

/// Smooth frecency weight for `(count, last_unix_secs)` at timestamp `now_secs`, using an
/// exponential 7-day half-life decay so recent and frequent items rank highest while older
/// items decay smoothly without cliff drops.
pub fn frecency_weigh(count: u32, last_secs: u64, now_secs: u64) -> u32 {
    if count == 0 {
        return 0;
    }
    let days = now_secs.saturating_sub(last_secs) as f64 / 86_400.0;
    let decay = (0.5_f64).powf(days / 7.0).max(0.1);
    let boost = if days < 0.25 { 4.5 } else { 4.0 * decay };
    (f64::from(count.min(100)) * boost * 10.0).round() as u32
}

#[derive(Serialize, Deserialize, Default, Clone, Copy, Debug)]
pub struct UseRecord {
    pub count: u32,
    pub last: u64,
}

/// Persisted frecency table (`key -> UseRecord`) backed by a JSON file in `~/.config/chickenwm/`.
pub struct FrecencyStore {
    path: PathBuf,
    uses: HashMap<String, UseRecord>,
}

impl FrecencyStore {
    /// Loads (or initializes) a frecency store at `path`.
    pub fn load_from(path: PathBuf) -> Self {
        let uses = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self { path, uses }
    }

    /// Loads the default launcher frecency store (`~/.config/chickenwm/launcher-frecency.json`).
    pub fn load() -> Self {
        Self::load_from(crate::config::config_dir().join("launcher-frecency.json"))
    }

    /// Records a use of `key` now and persists the store.
    pub fn record(&mut self, key: &str) {
        if key.is_empty() {
            return;
        }
        let u = self.uses.entry(key.to_owned()).or_default();
        u.count += 1;
        u.last = now_unix_secs();
        self.save();
    }

    /// Removes `key` from frecency history ("forget this entry").
    pub fn forget(&mut self, key: &str) -> bool {
        let removed = self.uses.remove(key).is_some();
        if removed {
            self.save();
        }
        removed
    }

    /// Inserts or updates a raw `(count, last)` entry (useful for tests).
    pub fn insert_raw(&mut self, key: impl Into<String>, count: u32, last: u64) {
        self.uses.insert(key.into(), UseRecord { count, last });
    }

    fn save(&self) {
        if let Ok(s) = serde_json::to_string(&self.uses) {
            let _ = std::fs::write(&self.path, s);
        }
    }

    /// Frecency score for `key` at the current time (0 if never used).
    pub fn score(&self, key: &str) -> u32 {
        Self::weigh(self.uses.get(key).copied(), now_unix_secs())
    }

    /// Frecency score for an optional `UseRecord` at timestamp `t`.
    pub fn weigh(u: Option<UseRecord>, t: u64) -> u32 {
        let Some(u) = u else { return 0 };
        frecency_weigh(u.count, u.last, t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_ranks_word_starts_and_contiguous_higher() {
        assert!(fuzzy_score("dl", r"C:\Users\me\Downloads").is_some());
        assert!(fuzzy_score("xyz", r"C:\Users").is_none());
        let a = fuzzy_score("down", r"C:\Users\me\Downloads").unwrap();
        let b = fuzzy_score("down", r"C:\Users\me\docs\own").unwrap();
        assert!(a > b);
    }

    #[test]
    fn frecency_decays_smoothly_and_forgets() {
        let t = 10_000_000;
        let fresh = frecency_weigh(5, t - 60, t);
        let three_days = frecency_weigh(5, t - 3 * 86_400, t);
        let week_old = frecency_weigh(5, t - 7 * 86_400, t);
        let month_old = frecency_weigh(5, t - 40 * 86_400, t);
        assert!(fresh > three_days && three_days > week_old && week_old > month_old && month_old > 0);

        let mut store = FrecencyStore {
            path: std::env::temp_dir().join(format!("cwm-core-frecency-{}.json", std::process::id())),
            uses: HashMap::new(),
        };
        store.record("app:Firefox");
        assert!(store.score("app:Firefox") > 0);
        assert!(store.forget("app:Firefox"));
        assert_eq!(store.score("app:Firefox"), 0);
        let _ = std::fs::remove_file(&store.path);
    }
}

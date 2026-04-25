//! In-memory playlist cursor state machine.
//!
//! `PlaylistState` is the single source of truth for "which wallpaper
//! comes next". It owns:
//!   - the resolved id list for the active playlist (curated members,
//!     smart-filter materialization, or the "All" snapshot — the state
//!     itself does not care which);
//!   - the playback `mode` (sequential / shuffle / random);
//!   - the cursor / shuffle round / RNG that `step` uses to advance.
//!
//! The control layer (`crate::control`) is responsible for refreshing
//! `ids` whenever the underlying source changes (rescan, smart-filter
//! re-evaluation, library mutation) and for actually applying the
//! wallpaper that `step` returns.
//!
//! Construction is `Default::default()` — that yields the All
//! pseudo-playlist (no active id, sequential, empty ids).

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::filter::Filter;

/// Cursor advancement strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `cursor + delta` mod len. Stable order, simple `next`/`prev`.
    #[default]
    Sequential,
    /// Each round visits every id exactly once in a random
    /// permutation; on wrap a fresh permutation is generated whose
    /// first element is **not** the same as the just-played id.
    Shuffle,
    /// Independent uniform sample on every step (excluding the current
    /// id when `len > 1`). No history; `delta` sign is ignored.
    Random,
}

impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Sequential => "sequential",
            Mode::Shuffle => "shuffle",
            Mode::Random => "random",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "sequential" => Some(Mode::Sequential),
            "shuffle" => Some(Mode::Shuffle),
            "random" => Some(Mode::Random),
            _ => None,
        }
    }
}

/// Stored RNG seed for shuffle rounds. Persisted on the `playlist` row
/// so a daemon restart resumes the same sequence rather than burning a
/// fresh one. Zero is reserved for "auto-pick from now-ms on first use".
pub type ShuffleSeed = u64;

#[derive(Debug, Clone, Default)]
pub struct PlaylistState {
    /// DB id of the active playlist; `None` means the implicit
    /// "All" pseudo-playlist (every wallpaper the source manager
    /// currently knows about).
    pub active_id: Option<i64>,

    /// Cached filter for smart playlists. `None` for curated playlists
    /// and for the All pseudo-playlist. Stored here so `refresh_sources`
    /// can re-evaluate without round-tripping to the DB.
    pub filter: Option<Filter>,

    pub mode: Mode,

    /// Resolved id list, **after** filter / curated lookup. Driving
    /// loop reads only this — it never needs to know the source kind.
    pub ids: Vec<String>,

    /// Last id we actually applied. Persisted across `refresh()` so a
    /// rescan that doesn't drop the current item leaves the user
    /// looking at what they had.
    pub current: Option<String>,

    /// Sequential / Random: index into `ids`.
    pub cursor: usize,

    /// Permutation of `[0..ids.len())` for the current shuffle round.
    /// Empty = no round prepared yet (built lazily on first shuffle
    /// `step`).
    shuffle_order: Vec<usize>,
    /// Position within `shuffle_order`.
    shuffle_pos: usize,

    pub shuffle_seed: ShuffleSeed,
    /// Live RNG state. Initialized lazily from `shuffle_seed`.
    rng: u64,
}

impl PlaylistState {
    /// Replace `ids` with a freshly resolved list, then re-pin
    /// `cursor` / `shuffle_pos` to the previous `current` if it still
    /// exists. The shuffle round is invalidated — a subsequent shuffle
    /// `step` will rebuild it.
    pub fn refresh(&mut self, ids: Vec<String>) {
        self.ids = ids;
        // Anything that referenced indices into the old vec is stale.
        self.shuffle_order.clear();
        self.shuffle_pos = 0;
        if self.ids.is_empty() {
            self.cursor = 0;
            return;
        }
        // If `current` survived the refresh, point cursor at it; else
        // clamp cursor to the new bounds.
        let cur = self.current.clone();
        if let Some(id) = cur.as_deref() {
            if let Some(pos) = self.ids.iter().position(|x| x == id) {
                self.cursor = pos;
                return;
            }
        }
        if self.cursor >= self.ids.len() {
            self.cursor = 0;
        }
    }

    /// Find `id` in the resolved list and snap cursors to it.
    pub fn locate(&mut self, id: &str) {
        if let Some(pos) = self.ids.iter().position(|x| x == id) {
            self.cursor = pos;
            if !self.shuffle_order.is_empty() {
                if let Some(sp) = self.shuffle_order.iter().position(|&i| i == pos) {
                    self.shuffle_pos = sp;
                }
            }
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if mode == self.mode {
            return;
        }
        self.mode = mode;
        // Switching into shuffle invalidates any prior round.
        self.shuffle_order.clear();
        self.shuffle_pos = 0;
    }

    /// Update active selection. Caller is responsible for refilling
    /// `ids` afterwards (via [`refresh`]). Resets the shuffle round
    /// because positions in the new list have nothing to do with
    /// positions in the old.
    pub fn set_active(&mut self, active_id: Option<i64>, filter: Option<Filter>) {
        self.active_id = active_id;
        self.filter = filter;
        self.shuffle_order.clear();
        self.shuffle_pos = 0;
    }

    pub fn count(&self) -> usize {
        self.ids.len()
    }

    /// 0-indexed display position of `current` in `ids`, if any.
    pub fn position(&self) -> Option<usize> {
        let id = self.current.as_deref()?;
        self.ids.iter().position(|x| x == id)
    }

    /// Advance the cursor by `delta` according to the active mode and
    /// return the resulting id, or `None` if the playlist is empty.
    /// Sets `current` to the returned id as a side-effect.
    pub fn step(&mut self, delta: i32) -> Option<String> {
        if self.ids.is_empty() {
            return None;
        }
        let len = self.ids.len();
        let chosen = match self.mode {
            Mode::Sequential => self.step_sequential(delta, len),
            Mode::Shuffle => self.step_shuffle(delta, len),
            Mode::Random => self.step_random(len),
        };
        let id = self.ids[chosen].clone();
        self.cursor = chosen;
        self.current = Some(id.clone());
        Some(id)
    }

    fn step_sequential(&mut self, delta: i32, len: usize) -> usize {
        let cur = self.cursor as i64;
        let n = len as i64;
        let next = ((cur + delta as i64) % n + n) % n;
        next as usize
    }

    fn step_shuffle(&mut self, delta: i32, len: usize) -> usize {
        // Freshly entered shuffle (or post-refresh invalidation): build
        // a round and land on slot 0 directly so the first step
        // actually plays `shuffle_order[0]` instead of skipping it.
        if self.shuffle_order.len() != len {
            self.build_shuffle_round(None, 0);
            self.shuffle_pos = 0;
            return self.shuffle_order[0];
        }
        let n = self.shuffle_order.len() as i64;
        let pos = self.shuffle_pos as i64;
        let raw = pos + delta as i64;
        if raw >= n {
            // Wrap forward: new round, slot 0 must differ from current.
            self.build_shuffle_round(Some(self.cursor), 0);
            self.shuffle_pos = 0;
        } else if raw < 0 {
            // Wrap backward: new round, the slot we land on (last)
            // must differ from current.
            let target = self.ids.len().saturating_sub(1);
            self.build_shuffle_round(Some(self.cursor), target);
            self.shuffle_pos = target;
        } else {
            self.shuffle_pos = raw as usize;
        }
        self.shuffle_order[self.shuffle_pos]
    }

    fn step_random(&mut self, len: usize) -> usize {
        if len == 1 {
            return 0;
        }
        let cur = self.cursor;
        loop {
            let pick = self.rng_range(len as u32) as usize;
            if pick != cur {
                return pick;
            }
        }
    }

    /// Build a fresh permutation of `[0..ids.len())` into
    /// `shuffle_order`. If `avoid` is `Some`, the slot at
    /// `target_pos` is guaranteed not to equal it — used so that
    /// wrap-forward (target=0) and wrap-backward (target=len-1) never
    /// replay the just-played id as the very next step.
    fn build_shuffle_round(&mut self, avoid: Option<usize>, target_pos: usize) {
        let n = self.ids.len();
        self.shuffle_order = (0..n).collect();
        // Fisher-Yates.
        for i in (1..n).rev() {
            let j = self.rng_range((i + 1) as u32) as usize;
            self.shuffle_order.swap(i, j);
        }
        if let Some(av) = avoid {
            if n > 1 && self.shuffle_order.get(target_pos).copied() == Some(av) {
                // Pick any other slot uniformly and swap with target.
                let raw = self.rng_range((n - 1) as u32) as usize;
                let alt = if raw >= target_pos { raw + 1 } else { raw };
                self.shuffle_order.swap(target_pos, alt);
            }
        }
    }

    // ---- xorshift64 RNG ----

    fn rng_state(&mut self) -> &mut u64 {
        if self.rng == 0 {
            // Seed precedence: explicit shuffle_seed, else now-ms, else
            // a fixed non-zero fallback so even a frozen clock works.
            let seed = if self.shuffle_seed != 0 {
                self.shuffle_seed
            } else {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0)
            };
            self.rng = if seed == 0 { 0xdead_beef_cafe_babe } else { seed };
        }
        &mut self.rng
    }

    fn rng_next(&mut self) -> u64 {
        let s = self.rng_state();
        let mut x = *s;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *s = x;
        x
    }

    fn rng_range(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        (self.rng_next() % n as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_with_ids(ids: &[&str]) -> PlaylistState {
        let mut s = PlaylistState::default();
        s.refresh(ids.iter().map(|x| x.to_string()).collect());
        s
    }

    #[test]
    fn empty_state_step_returns_none() {
        let mut s = PlaylistState::default();
        assert_eq!(s.step(1), None);
        assert_eq!(s.step(-1), None);
    }

    #[test]
    fn sequential_wraps_forward_and_back() {
        let mut s = fresh_with_ids(&["a", "b", "c"]);
        assert_eq!(s.step(1).as_deref(), Some("b"));
        assert_eq!(s.step(1).as_deref(), Some("c"));
        assert_eq!(s.step(1).as_deref(), Some("a"));
        assert_eq!(s.step(-1).as_deref(), Some("c"));
        assert_eq!(s.step(-1).as_deref(), Some("b"));
    }

    #[test]
    fn refresh_keeps_cursor_pinned_to_current_when_present() {
        let mut s = fresh_with_ids(&["a", "b", "c"]);
        s.step(1); // -> "b"
        assert_eq!(s.current.as_deref(), Some("b"));
        s.refresh(vec!["x".into(), "b".into(), "y".into()]);
        // cursor must point at "b" in the new list (idx 1).
        assert_eq!(s.cursor, 1);
        // Step forward should land on the next id after b.
        assert_eq!(s.step(1).as_deref(), Some("y"));
    }

    #[test]
    fn refresh_drops_unknown_current_and_clamps_cursor() {
        let mut s = fresh_with_ids(&["a", "b", "c"]);
        s.step(1); // -> "b"
        s.refresh(vec!["x".into(), "y".into()]);
        // current "b" no longer present; cursor preserved if in range.
        assert!(s.cursor < 2);
    }

    #[test]
    fn locate_snaps_cursor() {
        let mut s = fresh_with_ids(&["a", "b", "c", "d"]);
        s.locate("c");
        assert_eq!(s.cursor, 2);
        assert_eq!(s.step(1).as_deref(), Some("d"));
    }

    #[test]
    fn shuffle_round_visits_every_id_exactly_once() {
        let mut s = fresh_with_ids(&["a", "b", "c", "d", "e"]);
        s.shuffle_seed = 42;
        s.set_mode(Mode::Shuffle);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let id = s.step(1).unwrap();
            assert!(seen.insert(id), "shuffle round repeated an id");
        }
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn shuffle_reshuffles_on_wrap_and_avoids_immediate_repeat() {
        let mut s = fresh_with_ids(&["a", "b", "c", "d"]);
        s.shuffle_seed = 7;
        s.set_mode(Mode::Shuffle);
        // Walk full rounds and ensure no two consecutive steps return
        // the same id (catches the wrap-replay bug).
        let mut last: Option<String> = None;
        for _ in 0..16 {
            let id = s.step(1).unwrap();
            assert!(last.as_deref() != Some(id.as_str()));
            last = Some(id);
        }
    }

    #[test]
    fn random_never_repeats_immediately_when_len_gt_1() {
        let mut s = fresh_with_ids(&["a", "b", "c"]);
        s.shuffle_seed = 11;
        s.set_mode(Mode::Random);
        let mut last = s.step(1).unwrap();
        for _ in 0..50 {
            let next = s.step(1).unwrap();
            assert_ne!(next, last, "random returned the just-played id");
            assert_eq!(s.current.as_deref(), Some(next.as_str()));
            last = next;
        }
    }

    #[test]
    fn random_with_single_id_returns_that_id() {
        let mut s = fresh_with_ids(&["only"]);
        s.set_mode(Mode::Random);
        assert_eq!(s.step(1).as_deref(), Some("only"));
        assert_eq!(s.step(7).as_deref(), Some("only"));
    }

    #[test]
    fn set_mode_resets_shuffle_round() {
        let mut s = fresh_with_ids(&["a", "b", "c"]);
        s.shuffle_seed = 1;
        s.set_mode(Mode::Shuffle);
        s.step(1);
        assert!(!s.shuffle_order.is_empty());
        s.set_mode(Mode::Sequential);
        assert!(s.shuffle_order.is_empty());
        s.set_mode(Mode::Shuffle);
        assert!(s.shuffle_order.is_empty()); // built lazily
    }

    #[test]
    fn position_reports_current_index() {
        let mut s = fresh_with_ids(&["a", "b", "c"]);
        s.step(1);
        s.step(1);
        assert_eq!(s.position(), Some(2));
    }

    #[test]
    fn deterministic_seed_yields_same_shuffle_sequence() {
        fn play(seed: u64) -> Vec<String> {
            let mut s = fresh_with_ids(&["a", "b", "c", "d", "e"]);
            s.shuffle_seed = seed;
            s.set_mode(Mode::Shuffle);
            (0..5).map(|_| s.step(1).unwrap()).collect()
        }
        assert_eq!(play(123), play(123));
        assert_ne!(play(123), play(124));
    }

    #[test]
    fn mode_str_roundtrip() {
        for m in [Mode::Sequential, Mode::Shuffle, Mode::Random] {
            assert_eq!(Mode::from_str(m.as_str()), Some(m));
        }
        assert_eq!(Mode::from_str("nonsense"), None);
    }
}

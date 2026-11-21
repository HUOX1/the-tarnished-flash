//! read-only poller for the player's SpEffect chain.
//!
//! Purpose: turn "the player used 神奇素" into one rising-edge event, without
//! touching anything. The signal is SpEffect id 34 (confirmed live by the K
//! probe: duration 0.10 s, observed lifetime ~0.15 s).
//!
//! Hard rules this module follows, all from the L task book:
//!   * READ ONLY. `Backend::read_*` only; there is no write call in this file.
//!   * Every pointer is validated before use: image-range check on the vtable,
//!     owner back-reference on the SpecialEffect, bounded traversal with cycle
//!     detection. A failed check means "no sample", never "assume absent".
//!   * The player pointer is re-resolved on EVERY poll. Death, reload, fast
//!     travel and character swaps all invalidate a cached pointer, and a stale
//!     one is exactly how you read freed memory.
//!   * Rising edge only. A signal that lingers never re-fires.
//!   * The first successful sample after start-up / player change is a BASELINE,
//!     not an edge: if id 34 is already present we must not fire on old state.
//!
//! The detector deliberately owns no activation logic. It reports an event; the
//! caller decides (and the existing `request_activation(Item)` gate decides the
//! rest). That keeps the verified slowdown core untouched.

use crate::core::{Backend, Config};

// ---- ChrIns / SpecialEffect / entry layout (K, live-verified) ----------
pub const OFF_SPEFFECT: usize = 0x178;

pub const SE_OFF_VTABLE: usize = 0x00;
pub const SE_OFF_HEAD: usize = 0x08;
pub const SE_OFF_OWNER: usize = 0x10;
pub const SE_LEN: usize = 0x20;

pub const ENT_OFF_PARAM_ID: usize = 0x08;
pub const ENT_OFF_NEXT: usize = 0x30;
/// Little endian layout is contiguous from 0x00 to 0x38, so one read per node.
pub const ENT_READ_LEN: usize = 0x38;

/// Chain traversal budget. Live chains hold 5-10 entries; 64 is far above that
/// and still bounds a corrupted list.
pub const MAX_CHAIN: usize = 64;

/// Same image window the core uses for its vtable checks.
const IMAGE_WINDOW: usize = 0x0800_0000;

/// What one poll produced. The caller maps this to logging / activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Poll {
    /// Disabled, or the poll interval has not elapsed yet.
    Skipped,
    /// First successful sample after arming: baseline only, never an edge.
    Baseline { present: bool },
    /// Absent -> present. This is the one case that may start a slowdown.
    RisingEdge,
    /// A normal sample with no transition.
    Sampled { present: bool },
    /// The structure could not be read. Nothing is inferred from this.
    Unavailable(&'static str),
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub polls: u64,
    pub sampled: u64,
    pub unavailable: u64,
    pub edges: u64,
    pub baselines: u64,
    pub player_changes: u64,
    pub reads: u64,
    pub read_failures: u64,
    pub chain_max: usize,
    pub last_interval_ms: u64,
    pub max_interval_ms: u64,
}

pub struct Detector {
    pub enabled: bool,
    pub signal_id: i32,
    pub poll_ms: u64,
    next_due_ms: u64,
    last_poll_ms: Option<u64>,
    prev_present: bool,
    armed: bool,
    last_player: usize,
    last_reason: Option<&'static str>,
    pub stats: Stats,
}

impl Detector {
    pub fn new(enabled: bool, signal_id: i32, poll_ms: u64) -> Self {
        Detector {
            enabled,
            signal_id,
            poll_ms: poll_ms.max(1),
            next_due_ms: 0,
            last_poll_ms: None,
            prev_present: false,
            armed: false,
            last_player: 0,
            last_reason: None,
            stats: Stats::default(),
        }
    }

    /// True when the reason changed since the last poll, so the caller can log
    /// once per state change instead of once per tick.
    pub fn take_new_reason(&mut self, reason: &'static str) -> bool {
        if self.last_reason == Some(reason) {
            return false;
        }
        self.last_reason = Some(reason);
        true
    }

    pub fn prev_present(&self) -> bool {
        self.prev_present
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    pub fn last_reason(&self) -> Option<&'static str> {
        self.last_reason
    }

    /// One poll. Never writes, never panics, never blocks.
    pub fn poll<B: Backend>(&mut self, b: &mut B, cfg: &Config, now_ms: u64) -> Poll {
        if !self.enabled {
            return Poll::Skipped;
        }
        if now_ms < self.next_due_ms {
            return Poll::Skipped;
        }
        self.next_due_ms = now_ms.saturating_add(self.poll_ms);
        self.stats.polls += 1;

        if let Some(prev) = self.last_poll_ms {
            let dt = now_ms.saturating_sub(prev);
            self.stats.last_interval_ms = dt;
            if dt > self.stats.max_interval_ms {
                self.stats.max_interval_ms = dt;
            }
        }
        self.last_poll_ms = Some(now_ms);

        let (base, player) = match resolve_player(b, cfg) {
            Ok(v) => v,
            Err(r) => {
                self.stats.unavailable += 1;
                return Poll::Unavailable(r);
            }
        };

        if player != self.last_player {
            // A different character object: drop every cached assumption and
            // re-arm. 34 may already be present on the new object; that is old
            // state, not a fresh use.
            // The very first resolution (0 -> player) is not a "change" - if it
            // were, every start-up would report a phantom character swap.
            if self.last_player != 0 {
                self.stats.player_changes += 1;
            }
            self.last_player = player;
            self.armed = false;
            self.prev_present = false;
        }

        let present = match has_signal(b, base, player, self.signal_id, &mut self.stats) {
            Ok(v) => v,
            Err(r) => {
                self.stats.unavailable += 1;
                return Poll::Unavailable(r);
            }
        };
        self.stats.sampled += 1;
        self.last_reason = None;

        if !self.armed {
            self.armed = true;
            self.prev_present = present;
            self.stats.baselines += 1;
            return Poll::Baseline { present };
        }

        let was = self.prev_present;
        self.prev_present = present;
        if present && !was {
            self.stats.edges += 1;
            return Poll::RisingEdge;
        }
        Poll::Sampled { present }
    }
}

/// Resolve the live player ChrIns. Re-done every poll on purpose.
fn resolve_player<B: Backend>(b: &mut B, cfg: &Config) -> Result<(usize, usize), &'static str> {
    let base = b.module_base();
    if base == 0 {
        return Err("MODULE_BASE_UNAVAILABLE");
    }
    let slot = base.checked_add(cfg.wcm_rva).ok_or("WCM_RVA_OVERFLOW")?;
    let wcm = b.read_u64(slot).ok_or("WCM_UNREADABLE")? as usize;
    if wcm == 0 {
        return Err("WCM_NULL");
    }
    let mut player = b.read_u64(wcm + cfg.player_off).unwrap_or(0) as usize;
    if player == 0 {
        player = cfg.player_chr_ptr;
    }
    if player == 0 {
        return Err("PLAYER_NULL");
    }
    let vt = b.read_u64(player).ok_or("PLAYER_VTABLE_UNREADABLE")?;
    if !in_image(base, vt) {
        return Err("PLAYER_VTABLE_OUTSIDE_IMAGE");
    }
    Ok((base, player))
}

#[inline]
fn in_image(base: usize, ptr: u64) -> bool {
    let p = ptr as usize;
    p >= base && p < base.saturating_add(IMAGE_WINDOW)
}

/// Walk the player's SpEffect chain looking for `id`.
///
/// Returns Ok(true/false) only when the structure validated end to end. Any
/// anomaly returns Err, which the caller treats as "no sample" - never as
/// "absent", because guessing here would either miss a trigger or fire on junk.
fn has_signal<B: Backend>(
    b: &mut B,
    base: usize,
    player: usize,
    id: i32,
    stats: &mut Stats,
) -> Result<bool, &'static str> {
    let se = b
        .read_u64(player + OFF_SPEFFECT)
        .ok_or("SPEFFECT_PTR_UNREADABLE")?;
    if se == 0 {
        return Err("SPEFFECT_NULL");
    }
    let se = se as usize;
    let head = b.read_bytes(se, SE_LEN).ok_or("SPEFFECT_UNREADABLE")?;
    stats.reads += 1;
    let vtable = u64_at(&head, SE_OFF_VTABLE);
    if !in_image(base, vtable) {
        return Err("SPEFFECT_VTABLE_OUTSIDE_IMAGE");
    }
    if u64_at(&head, SE_OFF_OWNER) != player as u64 {
        return Err("SPEFFECT_OWNER_MISMATCH");
    }
    let mut node = u64_at(&head, SE_OFF_HEAD) as usize;
    if node == 0 {
        return Ok(false);
    }

    let mut visited: Vec<usize> = Vec::with_capacity(8);
    let mut steps = 0usize;
    while node != 0 && steps < MAX_CHAIN {
        if visited.contains(&node) {
            return Err("SPEFF_CHAIN_CYCLE");
        }
        visited.push(node);
        steps += 1;

        let blk = match b.read_bytes(node, ENT_READ_LEN) {
            Some(v) => v,
            None => {
                stats.read_failures += 1;
                return Err("SPEFF_ENTRY_UNREADABLE");
            }
        };
        stats.reads += 1;
        if i32_at(&blk, ENT_OFF_PARAM_ID) == id {
            if steps > stats.chain_max {
                stats.chain_max = steps;
            }
            return Ok(true);
        }
        node = u64_at(&blk, ENT_OFF_NEXT) as usize;
    }
    if steps > stats.chain_max {
        stats.chain_max = steps;
    }
    if node != 0 {
        // Ran out of budget with nodes left: the chain is not trustworthy.
        return Err("SPEFF_CHAIN_TOO_LONG");
    }
    Ok(false)
}

#[inline]
fn u64_at(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(v)
}

#[inline]
fn i32_at(b: &[u8], o: usize) -> i32 {
    let mut v = [0u8; 4];
    v.copy_from_slice(&b[o..o + 4]);
    i32::from_le_bytes(v)
}

// ---------------------------------------------------------------------------
// Offline tests. No game, no real memory.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const BASE: usize = 0x1400_0000_00;
    const WCM: usize = 0x2000_0000;
    const PLAYER: usize = 0x2100_0000;
    const SE: usize = 0x2200_0000;
    const ENT0: usize = 0x2300_0000;

    struct Mock {
        mem: HashMap<usize, Vec<u8>>,
        base: usize,
        now: u64,
        writes: usize,
    }

    impl Mock {
        fn new() -> Self {
            let mut m = Mock { mem: HashMap::new(), base: BASE, now: 0, writes: 0 };
            m.put_u64(BASE + crate::core::DEFAULT_WCM_RVA, WCM as u64);
            m.put_u64(WCM + crate::core::DEFAULT_PLAYER_OFF, PLAYER as u64);
            m.put_u64(PLAYER, (BASE + 0x9000) as u64); // player vtable in image
            m
        }
        fn put(&mut self, a: usize, v: &[u8]) {
            self.mem.insert(a, v.to_vec());
        }
        fn put_u64(&mut self, a: usize, v: u64) {
            self.put(a, &v.to_le_bytes());
        }
        /// Build a chain of `ids` and point ChrIns+0x178 at it.
        fn set_effects(&mut self, ids: &[i32]) {
            self.put_u64(PLAYER + OFF_SPEFFECT, SE as u64);
            let mut se = vec![0u8; SE_LEN];
            se[SE_OFF_VTABLE..SE_OFF_VTABLE + 8]
                .copy_from_slice(&((BASE + 0xA000) as u64).to_le_bytes());
            let head = if ids.is_empty() { 0 } else { ENT0 as u64 };
            se[SE_OFF_HEAD..SE_OFF_HEAD + 8].copy_from_slice(&head.to_le_bytes());
            se[SE_OFF_OWNER..SE_OFF_OWNER + 8].copy_from_slice(&(PLAYER as u64).to_le_bytes());
            self.put(SE, &se);

            let addr = |i: usize| ENT0 + i * 0x100;
            for (i, id) in ids.iter().enumerate() {
                let mut e = vec![0u8; ENT_READ_LEN];
                e[ENT_OFF_PARAM_ID..ENT_OFF_PARAM_ID + 4]
                    .copy_from_slice(&id.to_le_bytes());
                let next = if i + 1 < ids.len() { addr(i + 1) as u64 } else { 0 };
                e[ENT_OFF_NEXT..ENT_OFF_NEXT + 8].copy_from_slice(&next.to_le_bytes());
                self.put(addr(i), &e);
            }
        }
    }

    impl Backend for Mock {
        fn now_ms(&self) -> u64 {
            self.now
        }
        fn key_down(&self) -> bool {
            false
        }
        fn target_ptr(&mut self) -> usize {
            0
        }
        fn module_base(&mut self) -> usize {
            self.base
        }
        fn read_bytes(&mut self, a: usize, len: usize) -> Option<Vec<u8>> {
            match self.mem.get(&a) {
                Some(v) if v.len() >= len => Some(v[..len].to_vec()),
                _ => None,
            }
        }
        fn write_f32(&mut self, _a: usize, _v: f32) -> Result<(), ()> {
            self.writes += 1;
            Err(())
        }
        fn writable(&self, _a: usize) -> bool {
            false
        }
    }

    fn cfg() -> Config {
        let mut c = crate::core::Config {
            armed: true,
            player_chr_ptr: 0,
            speed_offset: crate::core::DEFAULT_SPEED_OFFSET,
            scale: 0.2,
            duration_ms: 15000,
            max_targets: 128,
            max_list_nodes: 256,
            wcm_rva: crate::core::DEFAULT_WCM_RVA,
            max_candidates: crate::core::MAX_CANDIDATE_SCAN,
            player_off: crate::core::DEFAULT_PLAYER_OFF,
            target_limit: 0,
            radius_m: 80.0,
            page_check: false,
            hold_probe_targets: 0,
            hotkey_enabled: true,
            debug_log: true,
            forced_halt: None,
        };
        c.player_chr_ptr = 0;
        c
    }

    fn det() -> Detector {
        Detector::new(true, 34, 10)
    }

    #[test]
    fn d1_baseline_with_signal_present_does_not_fire() {
        let mut m = Mock::new();
        m.set_effects(&[26, 34, 790]);
        let mut d = det();
        // start-up with 34 already on the player: baseline, not an edge
        assert_eq!(d.poll(&mut m, &cfg(), 100), Poll::Baseline { present: true });
        assert_eq!(d.stats.edges, 0, "old state must never fire");
        // still present, later polls: no edge either
        assert_eq!(d.poll(&mut m, &cfg(), 200), Poll::Sampled { present: true });
        assert_eq!(d.stats.edges, 0);
    }

    #[test]
    fn d2_absent_then_present_is_exactly_one_edge() {
        let mut m = Mock::new();
        m.set_effects(&[26, 790]);
        let mut d = det();
        assert_eq!(d.poll(&mut m, &cfg(), 100), Poll::Baseline { present: false });

        m.set_effects(&[26, 34, 790]); // 神奇素 used
        assert_eq!(d.poll(&mut m, &cfg(), 200), Poll::RisingEdge);
        assert_eq!(d.stats.edges, 1);
        // the 0.10 s signal is still there on the next sample: no second edge
        assert_eq!(d.poll(&mut m, &cfg(), 300), Poll::Sampled { present: true });
        assert_eq!(d.stats.edges, 1);

        m.set_effects(&[26, 790]); // signal expired
        assert_eq!(d.poll(&mut m, &cfg(), 400), Poll::Sampled { present: false });
        m.set_effects(&[26, 34, 790]); // used again
        assert_eq!(d.poll(&mut m, &cfg(), 500), Poll::RisingEdge);
        assert_eq!(d.stats.edges, 2, "re-armed after the signal disappeared");
    }

    #[test]
    fn d3_signal_lingering_never_re_fires() {
        let mut m = Mock::new();
        m.set_effects(&[]);
        let mut d = det();
        d.poll(&mut m, &cfg(), 100);
        m.set_effects(&[34]);
        assert_eq!(d.poll(&mut m, &cfg(), 200), Poll::RisingEdge);
        for t in 1..20 {
            assert_eq!(d.poll(&mut m, &cfg(), 200 + t * 100), Poll::Sampled { present: true });
        }
        assert_eq!(d.stats.edges, 1, "level would have fired 19 times");
    }

    #[test]
    fn d4_null_manager_is_unavailable_not_absent() {
        let mut m = Mock::new();
        m.put_u64(PLAYER + OFF_SPEFFECT, 0);
        let mut d = det();
        assert_eq!(d.poll(&mut m, &cfg(), 100), Poll::Unavailable("SPEFFECT_NULL"));
        assert_eq!(d.stats.sampled, 0);
        assert_eq!(d.stats.edges, 0);
        assert!(!d.armed(), "an unavailable read must not arm the edge detector");
    }

    #[test]
    fn d5_wrong_owner_is_rejected() {
        let mut m = Mock::new();
        m.set_effects(&[34]);
        // owner back-reference now points somewhere else
        let mut se = m.mem.get(&SE).unwrap().clone();
        se[SE_OFF_OWNER..SE_OFF_OWNER + 8].copy_from_slice(&0xDEADu64.to_le_bytes());
        m.put(SE, &se);
        let mut d = det();
        assert_eq!(
            d.poll(&mut m, &cfg(), 100),
            Poll::Unavailable("SPEFFECT_OWNER_MISMATCH")
        );
        assert_eq!(d.stats.edges, 0);
    }

    #[test]
    fn d6_vtable_outside_image_is_rejected() {
        let mut m = Mock::new();
        m.set_effects(&[34]);
        let mut se = m.mem.get(&SE).unwrap().clone();
        se[SE_OFF_VTABLE..SE_OFF_VTABLE + 8].copy_from_slice(&0x1234u64.to_le_bytes());
        m.put(SE, &se);
        let mut d = det();
        assert_eq!(
            d.poll(&mut m, &cfg(), 100),
            Poll::Unavailable("SPEFFECT_VTABLE_OUTSIDE_IMAGE")
        );
    }

    #[test]
    fn d7_cycle_and_overlong_chain_are_rejected() {
        // cycle: entry 0 -> entry 0
        let mut m = Mock::new();
        m.set_effects(&[26, 34]);
        let mut e0 = m.mem.get(&ENT0).unwrap().clone();
        e0[ENT_OFF_NEXT..ENT_OFF_NEXT + 8].copy_from_slice(&(ENT0 as u64).to_le_bytes());
        m.put(ENT0, &e0);
        let mut d = det();
        assert_eq!(d.poll(&mut m, &cfg(), 100), Poll::Unavailable("SPEFF_CHAIN_CYCLE"));

        // overlong: 70 entries, none matching
        let mut m2 = Mock::new();
        let ids: Vec<i32> = (0..70).map(|i| 1000 + i).collect();
        m2.set_effects(&ids);
        let mut d2 = det();
        assert_eq!(d2.poll(&mut m2, &cfg(), 100), Poll::Unavailable("SPEFF_CHAIN_TOO_LONG"));
        assert!(d2.stats.chain_max <= MAX_CHAIN);
    }

    #[test]
    fn d8_player_change_re_baselines_instead_of_firing() {
        let mut m = Mock::new();
        m.set_effects(&[]);
        let mut d = det();
        d.poll(&mut m, &cfg(), 100);

        // the game swaps the player ChrIns (death / reload / fast travel) and the
        // new one already carries 34
        let new_player = 0x2150_0000;
        m.put_u64(WCM + crate::core::DEFAULT_PLAYER_OFF, new_player as u64);
        m.put_u64(new_player, (BASE + 0x9000) as u64);
        m.put_u64(new_player + OFF_SPEFFECT, SE as u64);
        // retarget the SpecialEffect owner to the new player
        let mut se = m.mem.get(&SE).unwrap().clone();
        se[SE_OFF_OWNER..SE_OFF_OWNER + 8].copy_from_slice(&(new_player as u64).to_le_bytes());
        // SE head must point at the new chain: set_effects(&[]) left it null.
        // Patch the SAME buffer that is written below, otherwise the later put
        // clobbers this back to zero (that bug cost one test run).
        se[SE_OFF_HEAD..SE_OFF_HEAD + 8].copy_from_slice(&(ENT0 as u64).to_le_bytes());
        // make the chain contain 34
        let mut e = vec![0u8; ENT_READ_LEN];
        e[ENT_OFF_PARAM_ID..ENT_OFF_PARAM_ID + 4].copy_from_slice(&34i32.to_le_bytes());
        m.put(ENT0, &e);
        m.put(SE, &se);

        assert_eq!(d.poll(&mut m, &cfg(), 200), Poll::Baseline { present: true });
        assert_eq!(d.stats.edges, 0, "new character's existing effect is not a use");
        assert_eq!(d.stats.player_changes, 1);
    }

    #[test]
    fn d9_poll_interval_is_respected_and_measured() {
        let mut m = Mock::new();
        m.set_effects(&[]);
        let mut d = det();
        assert_eq!(d.poll(&mut m, &cfg(), 100), Poll::Baseline { present: false });
        assert_eq!(d.poll(&mut m, &cfg(), 105), Poll::Skipped, "too early");
        assert_eq!(d.poll(&mut m, &cfg(), 110), Poll::Sampled { present: false });
        assert_eq!(d.stats.polls, 2);
        assert_eq!(d.stats.last_interval_ms, 10);
    }

    #[test]
    fn d10_disabled_detector_does_nothing() {
        let mut m = Mock::new();
        m.set_effects(&[34]);
        let mut d = Detector::new(false, 34, 10);
        assert_eq!(d.poll(&mut m, &cfg(), 100), Poll::Skipped);
        assert_eq!(d.stats.polls, 0);
        assert_eq!(d.stats.reads, 0, "a disabled detector must not even read");
    }

    #[test]
    fn d11_never_writes() {
        let mut m = Mock::new();
        m.set_effects(&[]);
        let mut d = det();
        for t in 0..50 {
            d.poll(&mut m, &cfg(), 100 + t * 20);
        }
        assert_eq!(m.writes, 0, "the detector must never call write_f32");
    }

    #[test]
    fn d12_missing_player_is_unavailable() {
        let mut m = Mock::new();
        m.put_u64(WCM + crate::core::DEFAULT_PLAYER_OFF, 0);
        let mut c = cfg();
        c.player_chr_ptr = 0;
        let mut d = det();
        assert_eq!(d.poll(&mut m, &c, 100), Poll::Unavailable("PLAYER_NULL"));
    }
}

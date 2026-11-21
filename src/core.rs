//! F core, performance + effect fix.
//!
//! Diagnosis from the 20-target live run (see README):
//!   * `started_ms` was captured BEFORE enumeration, so the 15 s budget was
//!     consumed by ~13.9 s of preparation; the actual slow lasted 1.094 s.
//!   * Enumeration took ~12.5 s and writes ~1.27 s: every field read did its
//!     own `VirtualQuery` (11+ kernel calls per target). That is the freeze.
//!
//! Fixes in this file:
//!   1. timer starts AFTER every write completes;
//!   2. one batched read per ChrIns instead of 7 separate reads;
//!   3. periodic identity check rotates ONE target per tick (was: all targets
//!      every 5 ticks), while restore still re-checks every target;
//!   4. hold probes: read the speed field at +0 / +0.5 s / +2 s / pre-restore
//!      for the first few targets, so "keeps 0.2 or overwritten" is answered
//!      by data;
//!   5. target_limit + prefer_npc_param_id for staged testing (1 -> 2 -> 5..8).
//!
//! The speed field (+0x17C8), identity credentials, snapshot comparison and
//! restore-by-original-value are UNCHANGED from the live-verified E.

pub const VK_F8: i32 = 0x77;
/// panic button. Restores every address this session wrote that still
/// holds our scale -- the manual escape hatch if a target was lost mid-window.
pub const VK_F9: i32 = 0x78;

pub const OFF_VTABLE: usize = 0x00;
pub const OFF_NPC_PARAM_ID: usize = 0x60;
pub const OFF_NPC_ID: usize = 0x64;
pub const OFF_CHR_TYPE: usize = 0x68;
pub const OFF_TEAM_TYPE: usize = 0x6C;
pub const OFF_CHARACTER_ID: usize = 0x188;
pub const OFF_MODULES: usize = 0x190;
pub const CHR_HEAD_LEN: usize = 0x1A0;
pub const SLOT_BEHAVIOR_OFF: usize = 0x28;
pub const BEHAVIOR_OFF_OWNER: usize = 0x08;

/// WorldChrMan singleton slot: the .data pointer that points at the object.
/// 1.17.2 / EXE 2.7.2.0. Was 0x3D65F88 on 1.16.1; O located the new one
/// through RTTI instead of guessing.
pub const DEFAULT_WCM_RVA: usize = 0x3D6D018;
/// `CS::WorldChrManImp` vtable RVA on 1.17.2. The object's first qword must
/// equal `module_base + this`. Anything else means the running exe is a build
/// these offsets were NOT derived from, so every read below would be fiction.
pub const WCM_VTABLE_RVA: usize = 0x2A509D0;
pub const DEFAULT_PLAYER_OFF: usize = 0x1E508;
/// 1.16.1 only. On 1.17.2 this field reads 0. Kept so the old value stays
/// documented; it is never read any more.
pub const LEGACY_CHR_LIST_OFF: usize = 0x1E688;
/// 1.17.2 character enumeration. Each of these WorldChrMan fields points at an
/// array of `CHR_ARRAY_SLOTS` 16-byte ChrSetEntry records:
/// `{ chr: *ChrIns, load_status: u8, update_type: u8, flags: u8, pad: [u8; 5] }`.
/// The three arrays are NOT subsets of each other (measured: the second adds
/// characters the first does not have), so enumeration takes their union.
pub const CHR_ARRAY_FIELDS: [usize; 3] = [0x1E6A0, 0x1E6A8, 0x1E6B0];
pub const CHR_ARRAY_SLOTS: usize = 1500;
pub const CHR_ENTRY_SIZE: usize = 16;
/// ONE definition of the maximum contiguous read, shared by the real
/// WinBackend and the offline Mock. PERF-V3 introduced this 0x1000 ceiling;
/// P asked for 24000 bytes in one call and was refused before any memory
/// API ran. Keeping two independent constants is how that bug survived 69
/// passing offline tests.
pub const MAX_READ_BLOCK: usize = 0x1000;

pub const DEFAULT_SPEED_OFFSET: usize = 0x17C8;
pub const ALT_SPEED_OFFSET: usize = 0x15C0;

pub const DEFAULT_SCALE: f32 = 0.2;
pub const DEFAULT_DURATION_MS: u64 = 15000;
pub const DEFAULT_MAX_TARGETS: usize = 128;
/// Absolute ceiling; `SPEEDFORCE_M4F_MAX_TARGETS` above this is refused.
pub const HARD_MAX_TARGETS: usize = 128;
pub const DEFAULT_MAX_LIST_NODES: usize = 256;
pub const TICK_MS: u64 = 16;
/// how often a live window re-scans for characters that appeared after
/// activation (boss spawns, streamed-in enemies). 0 disables the rescan.
pub const RESCAN_MS: u64 = 2500;
/// a scan that finds almost nothing in range is retried this many times,
/// with SCAN_RETRY_MS between attempts, before any write happens.
pub const SCAN_ATTEMPTS: u32 = 3;
pub const SCAN_RETRY_MS: u64 = 150;
/// discovery window: WorldChrMan fields probed for additional character
/// arrays (F10). The three known arrays sit at 0x1E6A0/0x1E6A8/0x1E6B0.
pub const PROBE_FIELD_LO: usize = 0x1E5A0;
pub const PROBE_FIELD_HI: usize = 0x1E7A0;
pub const VK_F10: i32 = 0x79;
pub const IMAGE_WINDOW: usize = 0x0800_0000;
pub const CHR_TYPE_NPC: i32 = 5;
/// bosses carry their own chr_type. Margit (ChrId 2130, NpcParam
/// 21300014) reads as 7, which used to be rejected by the `== 5` screen, so
/// bosses were never targeted at all. Everything else about a boss - vtable,
/// container, behavior owner back-reference - is identical to an NPC.
pub const CHR_TYPE_BOSS: i32 = 7;
pub const PLAYER_TEAM: u8 = 1;

// --- G world position (milestone A, live-verified on 2.6.1.0) ---------
/// ChrInsModuleContainer slot 13 -> CSChrPhysicsModule.
pub const SLOT_PHYSICS_OFF: usize = 0x68;
pub const PHYS_OFF_VTABLE: usize = 0x00;
pub const PHYS_OFF_OWNER: usize = 0x08;
pub const PHYS_OFF_DATA_MODULE: usize = 0x20;
pub const PHYS_OFF_POSITION: usize = 0x70;
pub const PHYS_BLOCK_LEN: usize = 0x100;
pub const CONTAINER_LEN: usize = 0xA0;
pub const DEFAULT_RADIUS_M: f32 = 30.0;
/// Hard ceiling on how many characters get a position read per activation.
/// Ceiling on how many DISTINCT ChrIns get verified per activation. The three
/// arrays can hold 4500 entries and a busy map really does have ~560 live
/// characters, so this must sit well above the old 256 - otherwise characters
/// are dropped before the distance filter runs, which would silently skip
/// enemies standing right next to the player.
pub const MAX_CANDIDATE_SCAN: usize = 2048;
pub const HOLD_PROBE_MS: [u64; 2] = [500, 2000];

pub trait Backend {
    fn now_ms(&self) -> u64;
    fn key_down(&self) -> bool;
    /// manual "sweep and undo" key. Default false keeps tests unchanged.
    fn sweep_key(&self) -> bool {
        false
    }
    /// manual "dump every candidate chr array" key. Used to find the
    /// container a boss lives in when it is not in the three known arrays.
    fn probe_key(&self) -> bool {
        false
    }
    fn target_ptr(&mut self) -> usize;
    fn module_base(&mut self) -> usize;
    /// One contiguous read. On Windows this is ONE ReadProcessMemory call, so a
    /// 0x1A0-byte ChrIns header costs a single kernel transition.
    fn read_bytes(&mut self, addr: usize, len: usize) -> Option<Vec<u8>>;
    fn read_u8(&mut self, a: usize) -> Option<u8> {
        self.read_bytes(a, 1).map(|v| v[0])
    }
    fn read_i32(&mut self, a: usize) -> Option<i32> {
        self.read_bytes(a, 4).map(|v| i32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn read_u32(&mut self, a: usize) -> Option<u32> {
        self.read_bytes(a, 4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn read_u64(&mut self, a: usize) -> Option<u64> {
        self.read_bytes(a, 8).map(|v| {
            u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]])
        })
    }
    fn read_f32(&mut self, a: usize) -> Option<f32> {
        self.read_bytes(a, 4).map(|v| f32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
    fn write_f32(&mut self, addr: usize, value: f32) -> Result<(), ()>;
    /// Only used to refuse writes to pages that are not writable. Called a small
    /// number of times per target, never per field.
    fn writable(&self, addr: usize) -> bool;
    /// Per-primitive cost counters. Default: no measurement (tests).
    fn perf(&self) -> PerfCounters { PerfCounters::default() }
    fn reset_perf(&self) {}
    /// CPU time consumed by this thread so far, in nanoseconds.
    fn thread_cpu_ns(&self) -> u64 { 0 }
}

/// Nanosecond totals per Win32 primitive. Wall clock, so a preempted thread
/// shows up here even when the syscall itself was cheap; `thread_cpu_ns` at
/// the same moment tells the two apart.
#[derive(Clone, Copy, Debug, Default)]
pub struct PerfCounters {
    pub rpm_n: u64,
    pub rpm_ns: u64,
    pub rpm_max_ns: u64,
    pub vq_n: u64,
    pub vq_ns: u64,
    pub vq_max_ns: u64,
    pub wpm_n: u64,
    pub wpm_ns: u64,
    pub wpm_max_ns: u64,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub armed: bool,
    pub player_chr_ptr: usize,
    pub speed_offset: usize,
    pub scale: f32,
    pub duration_ms: u64,
    pub max_targets: usize,
    pub max_list_nodes: usize,
    pub wcm_rva: usize,
    pub player_off: usize,
    /// Cost ceiling for verification, not a coverage limit. Exceeding it is
    /// logged as SCAN_TRUNCATED and counted in `dropped_scan`.
    pub max_candidates: usize,
    /// 0 = no limit. Staged testing: 1 -> 2 -> 5..8.
    pub target_limit: usize,
    /// Player-centred selection radius in metres (G). Only characters whose
    /// squared distance is <= radius^2 are ever considered.
    pub radius_m: f32,
    /// Re-enable the pre-write `VirtualQuery` page check. Off by default: a
    /// failed `WriteProcessMemory` already reports the same condition, so the
    /// query is redundant work on the critical path.
    pub page_check: bool,
    /// How many targets get hold probes (0 = none).
    pub hold_probe_targets: usize,
    /// whether the debug hotkey entry point is allowed to start an
    /// activation. The hotkey is one *trigger*; the slowdown core behind it is
    /// untouched. When false, a key edge is logged once and ignored.
    pub hotkey_enabled: bool,
    /// high-volume diagnostics (SPEED_HOLD / PERF) are dropped when
    /// `[General] DebugLog=0`. Writes, restores and summaries are never dropped.
    pub debug_log: bool,
    /// a non-empty reason means "halt instead of running with a value the
    /// operator did not ask for" (invalid INI / env input). This is how a bad
    /// config becomes a loud stop rather than a silent surprise.
    pub forced_halt: Option<&'static str>,
}

impl Config {
    pub fn valid_scale(&self) -> bool {
        self.scale.is_finite() && self.scale > 0.0 && self.scale <= 1.0
    }
    /// A radius of 0 would mean "everything", so it is explicitly invalid:
    /// refusing to activate beats silently degrading to a map-wide slowdown.
    pub fn valid_radius(&self) -> bool {
        self.radius_m.is_finite() && self.radius_m > 0.0 && self.radius_m <= 200.0
    }
}

/// where an activation request came from. Every entry point - the debug
/// hotkey today, item / weapon skill later - goes through the SAME
/// `request_activation`, so radius, scale, duration, target screen, rollback and
/// restore are identical no matter who asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerSource {
    DebugHotkey,
    Item,
    WeaponSkill,
}

impl TriggerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TriggerSource::DebugHotkey => "DebugHotkey",
            TriggerSource::Item => "Item",
            TriggerSource::WeaponSkill => "WeaponSkill",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Active,
    Halted(&'static str),
}

// `ExternalModified(f32)` carries the observed value, and f32 has no
// `Eq`. Nothing in the codebase needs Eq for this type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ObjectState {
    SameObject,
    ControlFailed(&'static str),
    Unknown(&'static str),
    /// the speed field no longer holds the value WE wrote. Another
    /// system (the game, another mod) changed it during the window. Restoring
    /// `original` on top of that would silently overwrite somebody else's
    /// state, so the restore is skipped and the fact is logged instead.
    ExternalModified(f32),
}

/// result of undoing a write at an address that had to be re-derived
/// because the cached `behavior + speed_offset` went stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReresolveOutcome {
    /// Written back at the newly derived address.
    Restored(usize),
    /// The field already holds the original value; nothing to do.
    AlreadyNormal(usize),
    /// The write itself failed at the new address.
    Failed(usize),
    /// Cannot prove it is still the same character; memory left untouched.
    Abandoned(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreOutcome {
    NotActive,
    Restored,
    RestoreFailed,
    WriteForbidden,
}

#[derive(Clone, Debug)]
pub struct ActivationSnapshot {
    pub chr_ptr: usize,
    pub vtable: u64,
    pub npc_param_id: i32,
    pub npc_id: i32,
    pub chr_type: i32,
    pub team_type: u8,
    pub character_id: u32,
    pub container: u64,
    pub behavior: u64,
    pub owner: u64,
    pub physics: u64,
    pub position: [f32; 3],
    pub distance_m: f32,
    pub speed_addr: usize,
    pub original: f32,
}

/// H accounting. Every stage that can lose a character records the loss, so
/// "30 m and nothing happened" can always be explained by a number instead of a
/// guess. The four counts the operator reads are, in order:
/// `inside_radius` (in-radius candidates) -> `verified` -> `written` ->
/// `dropped_cap` (in-radius, never verified because the cap was reached).
#[derive(Clone, Debug, Default)]
pub struct EnumReport {
    /// Slots examined across every character array.
    pub slots_scanned: usize,
    /// Arrays whose pointer was non-null and whose contents were read.
    pub arrays_read: usize,
    /// Arrays whose WorldChrMan field was null (set genuinely empty).
    pub arrays_empty: usize,
    /// Arrays whose pointer was set but the read failed.
    pub arrays_unreadable: usize,
    /// arrays discarded because their WCM field changed between the
    /// first read and the post-read re-check (area streaming).
    pub arrays_base_changed: usize,
    /// Entries skipped because that ChrIns was already collected.
    pub duplicates: usize,
    /// distinct ChrIns that resolve to the SAME speed address. Writing
    /// twice would make the second snapshot record the already-slowed value as
    /// "original" and permanently lose the real one. They are skipped before
    /// the first write happens.
    pub duplicate_speed_addrs: usize,
    pub skipped_player: usize,
    pub skipped_invalid: usize,
    /// ChrIns pointers collected from the walk (player and nulls already out).
    pub candidates: usize,
    pub positions_read: usize,
    /// diagnostics: skip-reason histogram, so a run where almost nothing
    /// lands in range can be explained by counts instead of an 8-sample guess.
    pub reason_counts: Vec<(&'static str, u32)>,
    /// diagnostics: (distance, npc_id, team) of the probed candidates,
    /// sorted ascending; the first entries are the closest characters alive.
    pub nearest: Vec<(f32, i32, u8)>,
    /// diagnostics: distance histogram 0-5 / 5-10 / 10-25 / 25-50 /
    /// 50-100 / >100 m. Shows at a glance whether anything is actually close.
    pub buckets: [usize; 6],
    /// diagnostics: raw xyz of the five nearest characters, to check the
    /// reported distance against the coordinates themselves.
    pub nearest_pos: Vec<([f32; 3], f32)>,
    /// diagnostics: characters that were rejected by the chr_type screen
    /// but whose position IS readable: (distance, chr_type, npc_id,
    /// npc_param). Bosses are the main reason this exists - if a boss carries a
    /// chr_type other than 5 it never becomes a target.
    pub non_npc_near: Vec<(f32, i32, i32, i32, usize)>,
    /// diagnostics: chr_type histogram over every candidate whose header
    /// was readable. Answers "what type is this boss" without guessing.
    pub type_counts: Vec<(i32, u32)>,
    pub inside_radius: usize,
    pub outside_radius: usize,
    /// Effective write budget for this activation.
    pub cap: usize,
    /// Candidates that never got a position read because of `MAX_CANDIDATE_SCAN`.
    pub dropped_scan: usize,
    /// In-radius candidates that were never verified because the cap was hit.
    pub dropped_cap: usize,
    /// Identity verification succeeded (>= `written` only if a write failed).
    pub verified: usize,
    /// Writes that actually landed and were recorded.
    pub written: usize,
    /// `npc_param_id <= 0` seen on an otherwise verified target. Auxiliary
    /// evidence only: it is counted, it never blocks a write by itself.
    pub aux_npc_param_zero: usize,
    pub skip_reasons: Vec<&'static str>,
}

/// A character that passed the cheap screen and has a verified world position.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub chr_ptr: usize,
    pub head: Vec<u8>,
    pub cblk: Vec<u8>,
    pub container: u64,
    pub physics: u64,
    pub position: [f32; 3],
    pub d2: f32,
}

/// Microsecond phase timers, printed once per activation / restore as a single
/// `TIMING` event, so diagnosis costs no hot-path I/O.
#[derive(Clone, Debug, Default)]
pub struct Timing {
    pub enum_us: u64,
    pub position_us: u64,
    pub verify_us: u64,
    pub total_us: u64,
    pub write_us: u64,
    pub periodic_us: u64,
    pub periodic_count: u64,
    pub restore_us: u64,
    pub activation_span_ms: u64,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub t_ms: u64,
    pub kind: &'static str,
    pub detail: String,
}

#[derive(Clone, Copy, Debug)]
pub struct WriteRecord {
    pub addr: usize,
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct HoldProbe {
    pub tag: &'static str,
    pub addr: usize,
    pub at_ms: u64,
    pub value: Option<f32>,
}

fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3],
                        b[o + 4], b[o + 5], b[o + 6], b[o + 7]])
}
fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
/// Split `total` bytes into contiguous `(offset, length)` chunks, each no
/// larger than `MAX_READ_BLOCK`. No overlap, no gap; the last chunk carries
/// the remainder (3520 for 24000 bytes).
pub fn array_chunk_plan(total: usize) -> Vec<(usize, usize)> {
    let mut plan = Vec::new();
    let mut off = 0usize;
    while off < total {
        let n = MAX_READ_BLOCK.min(total - off);
        plan.push((off, n));
        off += n;
    }
    plan
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayReadError {
    pub chunk_index: usize,
    pub offset: usize,
    pub len: usize,
    pub reason: &'static str,
}

/// Read one whole character array through the Backend abstraction in chunks
/// the real WinBackend accepts. ANY failed chunk discards the entire partial
/// buffer: the caller never sees a half-read array and cannot mistake it for
/// a complete one. The original slot parsing is deliberately untouched.
pub fn read_array_chunked<B: Backend>(
    backend: &mut B,
    base: usize,
    total: usize,
) -> Result<Vec<u8>, ArrayReadError> {
    if base == 0 {
        return Err(ArrayReadError {
            chunk_index: 0, offset: 0, len: MAX_READ_BLOCK.min(total),
            reason: "NULL_ARRAY_BASE",
        });
    }
    let mut out = Vec::with_capacity(total);
    for (idx, (off, n)) in array_chunk_plan(total).into_iter().enumerate() {
        let addr = match base.checked_add(off) {
            Some(a) => a,
            None => {
                return Err(ArrayReadError {
                    chunk_index: idx, offset: off, len: n,
                    reason: "ADDRESS_OVERFLOW",
                })
            }
        };
        match backend.read_bytes(addr, n) {
            Some(b) if b.len() == n => out.extend_from_slice(&b),
            Some(_) => {
                return Err(ArrayReadError {
                    chunk_index: idx, offset: off, len: n, reason: "SHORT_READ",
                })
            }
            None => {
                return Err(ArrayReadError {
                    chunk_index: idx, offset: off, len: n, reason: "READ_REJECTED",
                })
            }
        }
    }
    Ok(out)
}

/// claim a speed address for the current activation. Returns false if
/// it was already written, so the caller MUST skip the second write. Two
/// characters resolving to one address would otherwise make the second
/// verification read the already-slowed value as `original`, and restoring
/// would leave the field slowed forever.
// `#[inline(never)]` is deliberate. With LTO enabled, LLVM inlined this helper,
// proved the duplicate branch unreachable for the current call graph and erased
// it - including its log string - from the release DLL (verified: debug build
// had `DUPLICATE_SPEED_ADDR`, release did not). A safety net that only exists
// in the source is not a safety net. Keeping the call opaque preserves it.
#[inline(never)]
fn claim_speed_addr(seen: &mut std::collections::HashSet<usize>, addr: usize) -> bool {
    std::hint::black_box(seen.insert(addr))
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn position_at(b: &[u8], o: usize) -> [f32; 3] {
    [f32_at(b, o), f32_at(b, o + 4), f32_at(b, o + 8)]
}
fn finite_position(p: &[f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite() && v.abs() < 1.0e5)
}
fn dist2(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
}

/// one bucket in the chr_type census.
fn bump_type(list: &mut Vec<(i32, u32)>, ct: i32) {
    match list.iter_mut().find(|(k, _)| *k == ct) {
        Some(e) => e.1 += 1,
        None => list.push((ct, 1)),
    }
}

pub struct Controller<B: Backend> {
    pub cfg: Config,
    pub backend: B,
    pub state: State,
    pub events: Vec<Event>,
    pub writes: Vec<WriteRecord>,
    pub targets: Vec<ActivationSnapshot>,
    /// targets that lost their identity mid-window. Their cached
    /// `speed_addr` may be stale, but the character can still be resolved from
    /// `chr_ptr` at restore time. Kept here so a boss whose physics module got
    /// rebuilt does not stay slowed forever.
    pub displaced: Vec<ActivationSnapshot>,
    /// every (address, original) this controller ever wrote, for the
    /// manual sweep. Capped like `writes`.
    pub undo: Vec<(usize, f32)>,
    pub last_enum: Option<EnumReport>,
    pub timing: Timing,
    pub holds: Vec<HoldProbe>,
    prev_key: bool,
    prev_sweep: bool,
    prev_probe: bool,
    /// timestamp of the last mid-window rescan.
    last_rescan_ms: u64,
    perf_cpu_mark: u64,
    started_ms: u64,
    tick_count: u64,
    probe_flags: [bool; 2],
    hotkey_disabled_logged: bool,
    /// why the most recent activation request did not start. Read by
    /// the host so ITEM_TRIGGER_REJECTED can name the actual cause instead of
    /// only saying `started=0`.
    last_refusal: Option<String>,
}

impl<B: Backend> Controller<B> {
    pub fn new(cfg: Config, backend: B) -> Self {
        // a config error wins over everything else. Running with a value
        // the operator did not ask for is worse than not running at all.
        let bad = if let Some(r) = cfg.forced_halt {
            Some(r)
        } else if !cfg.armed {
            Some("NOT_ARMED")
        } else if !cfg.valid_scale() {
            Some("INVALID_SCALE")
        } else if cfg.duration_ms == 0 {
            Some("INVALID_DURATION")
        } else if cfg.max_targets == 0 || cfg.max_targets > HARD_MAX_TARGETS {
            Some("INVALID_MAX_TARGETS")
        } else if !cfg.valid_radius() {
            Some("INVALID_RADIUS")
        } else if cfg.speed_offset != DEFAULT_SPEED_OFFSET
            && cfg.speed_offset != ALT_SPEED_OFFSET
        {
            Some("UNSUPPORTED_SPEED_OFFSET")
        } else {
            None
        };
        let mut c = Self {
            cfg,
            backend,
            state: State::Idle,
            events: Vec::new(),
            writes: Vec::new(),
            targets: Vec::new(),
            displaced: Vec::new(),
            undo: Vec::new(),
            last_enum: None,
            timing: Timing::default(),
            holds: Vec::new(),
            prev_key: false,
            prev_sweep: false,
            prev_probe: false,
            last_rescan_ms: 0,
            perf_cpu_mark: 0,
            started_ms: 0,
            tick_count: 0,
            probe_flags: [false; 2],
            hotkey_disabled_logged: false,
            last_refusal: None,
        };
        match bad {
            Some(r) => c.halt(r),
            None => c.log("ARMED_IDLE", "waiting for F8; no memory written yet"),
        }
        c
    }

    /// the concrete reason the last `request_activation` failed, or
    /// `None` if it succeeded. This is what makes a failed item trigger
    /// diagnosable from one log line.
    pub fn last_refusal(&self) -> Option<&str> {
        self.last_refusal.as_deref()
    }

    /// True when the 15 s slowdown window is currently running. Used by the
    /// host to explain why a second pill was refused.
    pub fn is_active(&self) -> bool {
        self.state == State::Active
    }

    /// Short state name for dispatch logs, so the host does not need to know
    /// the internals of `State`.
    pub fn state_label(&self) -> &'static str {
        match self.state {
            State::Idle => "Idle",
            State::Active => "Active",
            State::Halted(_) => "Halted",
        }
    }

    /// let the host crate (lib.rs) write configuration diagnostics through
    /// the same event stream, so CONFIG_* / NOT_IMPLEMENTED lines land in the
    /// same file as MODE and HALT instead of a second log.
    pub fn emit(&mut self, kind: &'static str, detail: impl Into<String>) {
        self.log(kind, detail);
    }

    /// Kinds that only exist for diagnosis. `[General] DebugLog=0` drops them;
    /// anything that records a write, a restore, a refusal or a halt is never
    /// dropped, so the safety trail stays complete either way.
    const DEBUG_ONLY_KINDS: [&'static str; 4] = ["SPEED_HOLD", "PERF", "TIMING", "WRITE_SUMMARY"];

    fn log(&mut self, kind: &'static str, detail: impl Into<String>) {
        if !self.cfg.debug_log && Self::DEBUG_ONLY_KINDS.contains(&kind) {
            return;
        }
        if self.events.len() >= 512 {
            self.events.drain(0..256);
        }
        self.events.push(Event { t_ms: self.backend.now_ms(), kind, detail: detail.into() });
    }

    fn halt(&mut self, reason: &'static str) {
        self.log("HALT", reason);
        self.state = State::Halted(reason);
    }

    /// Measure one phase: wall clock from the caller, CPU delta since the last
    /// mark, and the per-primitive counters in between.
    fn begin_phase(&mut self) {
        self.backend.reset_perf();
        self.perf_cpu_mark = self.backend.thread_cpu_ns();
    }

    fn log_perf(&mut self, phase: &'static str, wall_ns: u64) {
        let p = self.backend.perf();
        let cpu = self.backend.thread_cpu_ns().saturating_sub(self.perf_cpu_mark);
        let per = |n: u64, t: u64| if n == 0 { 0 } else { t / n };
        self.log(
            "PERF",
            format!(
                "phase={} wall_ms={:.3} cpu_ms={:.3} rpm_n={} rpm_avg_us={:.2} \
                 rpm_max_us={:.2} vq_n={} vq_avg_us={:.2} vq_max_us={:.2} \
                 wpm_n={} wpm_avg_us={:.2} wpm_max_us={:.2}",
                phase,
                wall_ns as f64 / 1.0e6,
                cpu as f64 / 1.0e6,
                p.rpm_n,
                per(p.rpm_n, p.rpm_ns) as f64 / 1000.0,
                p.rpm_max_ns as f64 / 1000.0,
                p.vq_n,
                per(p.vq_n, p.vq_ns) as f64 / 1000.0,
                p.vq_max_ns as f64 / 1000.0,
                p.wpm_n,
                per(p.wpm_n, p.wpm_ns) as f64 / 1000.0,
                p.wpm_max_ns as f64 / 1000.0,
            ),
        );
    }

    /// Write, then read the same 4 bytes back and compare bit patterns.
    /// This replaces the success flag that `WriteProcessMemory` used to give
    /// us, now that the store is a direct in-process write. Cost: ~1 us.
    fn write_checked(&mut self, addr: usize, value: f32) -> bool {
        if self.backend.write_f32(addr, value).is_err() {
            return false;
        }
        match self.backend.read_f32(addr) {
            Some(v) => v.to_bits() == value.to_bits(),
            None => false,
        }
    }

    fn record_write(&mut self, addr: usize, value: f32) {
        if self.writes.len() >= 256 {
            self.writes.drain(0..128);
        }
        self.writes.push(WriteRecord { addr, value });
    }

    /// panic button. Walks every (address, original) this session wrote
    /// and undoes the ones that still hold exactly our scale. The value gate is
    /// what makes it safe: an address whose field reads anything else is not
    /// ours to touch anymore.
    pub fn force_restore_sweep(&mut self) -> usize {
        let undo = self.undo.clone();
        let mut seen: Vec<usize> = Vec::new();
        let mut checked = 0usize;
        let mut restored = 0usize;
        let mut skipped = 0usize;
        for (addr, original) in undo.iter().rev() {
            if seen.contains(addr) {
                continue;
            }
            seen.push(*addr);
            checked += 1;
            match self.backend.read_f32(*addr) {
                Some(v) if v.to_bits() == self.cfg.scale.to_bits() => {
                    if self.write_checked(*addr, *original) {
                        self.record_write(*addr, *original);
                        restored += 1;
                        self.log(
                            "SWEEP_RESTORED",
                            format!("addr={:#x} -> {}", addr, original),
                        );
                    } else {
                        skipped += 1;
                        self.log(
                            "SWEEP_WRITE_FAILED",
                            format!("addr={:#x}", addr),
                        );
                    }
                }
                _ => skipped += 1,
            }
        }
        self.log(
            "FORCE_RESTORE_SWEEP",
            format!("checked={} restored={} skipped={}", checked, restored, skipped),
        );
        restored
    }

    /// ONE batched read of the ChrIns header + one read of the module chain +
    /// one read of the speed field. Never writes.
    fn vtable_in_image(&self, base: usize, vt: u64) -> bool {
        vt != 0 && vt >= base as u64 && vt < base as u64 + IMAGE_WINDOW as u64
    }

    /// Player world position. Failure here REFUSES the activation on purpose:
    /// there is no fallback that would degrade into a map-wide slowdown.
    fn player_position(&mut self, player: usize, base: usize) -> Result<[f32; 3], &'static str> {
        let head = self
            .backend
            .read_bytes(player, CHR_HEAD_LEN)
            .ok_or("PLAYER_CHRINS_UNREADABLE")?;
        let container = u64_at(&head, OFF_MODULES);
        if container == 0 {
            return Err("PLAYER_CONTAINER_NULL");
        }
        let cblk = self
            .backend
            .read_bytes(container as usize, CONTAINER_LEN)
            .ok_or("PLAYER_CONTAINER_UNREADABLE")?;
        let physics = u64_at(&cblk, SLOT_PHYSICS_OFF);
        if physics == 0 {
            return Err("PLAYER_PHYSICS_NULL");
        }
        let pblk = self
            .backend
            .read_bytes(physics as usize, PHYS_BLOCK_LEN)
            .ok_or("PLAYER_PHYSICS_UNREADABLE")?;
        if !self.vtable_in_image(base, u64_at(&pblk, PHYS_OFF_VTABLE)) {
            return Err("PLAYER_PHYSICS_VTABLE_OUTSIDE_IMAGE");
        }
        if u64_at(&pblk, PHYS_OFF_OWNER) != player as u64 {
            return Err("PLAYER_PHYSICS_OWNER_MISMATCH");
        }
        if u64_at(&pblk, PHYS_OFF_DATA_MODULE) != u64_at(&cblk, 0) {
            return Err("PLAYER_PHYSICS_DATA_MODULE_MISMATCH");
        }
        let pos = position_at(&pblk, PHYS_OFF_POSITION);
        if !finite_position(&pos) {
            return Err("PLAYER_POSITION_NOT_FINITE");
        }
        Ok(pos)
    }

    /// Stages 3-5: cheap identity screen, then world position, then radius.
    /// Nothing expensive happens here, so a character outside the radius costs
    /// only three reads and never touches its BehaviorModule.
    fn probe_candidate(
        &mut self,
        chr_ptr: usize,
        player: usize,
        base: usize,
    ) -> Result<Candidate, &'static str> {
        let head = self
            .backend
            .read_bytes(chr_ptr, CHR_HEAD_LEN)
            .ok_or("CHRINS_UNREADABLE")?;
        // Live-verified: the ChrSet list HEAD can be a sentinel whose +0x00 is
        // not a ChrIns*, so screen by "vtable inside our image".
        if !self.vtable_in_image(base, u64_at(&head, OFF_VTABLE)) {
            return Err("NOT_CHRINS_OR_SENTINEL");
        }
        // Player exclusion is the ChrIns POINTER, nothing else. Team is not
        // consulted any more: friendly / neutral characters participate too.
        if chr_ptr == player {
            return Err("IS_PLAYER");
        }
        let chr_type = i32_at(&head, OFF_CHR_TYPE);
        if chr_type != CHR_TYPE_NPC && chr_type != CHR_TYPE_BOSS {
            return Err("NOT_NPC");
        }
        let container = u64_at(&head, OFF_MODULES);
        if container == 0 {
            return Err("NULL_CONTAINER");
        }
        let cblk = self
            .backend
            .read_bytes(container as usize, CONTAINER_LEN)
            .ok_or("CONTAINER_UNREADABLE")?;
        let physics = u64_at(&cblk, SLOT_PHYSICS_OFF);
        if physics == 0 {
            return Err("NULL_PHYSICS");
        }
        let pblk = self
            .backend
            .read_bytes(physics as usize, PHYS_BLOCK_LEN)
            .ok_or("PHYSICS_UNREADABLE")?;
        if !self.vtable_in_image(base, u64_at(&pblk, PHYS_OFF_VTABLE)) {
            return Err("PHYSICS_VTABLE_OUTSIDE_IMAGE");
        }
        if u64_at(&pblk, PHYS_OFF_OWNER) != chr_ptr as u64 {
            return Err("PHYSICS_OWNER_MISMATCH");
        }
        if u64_at(&pblk, PHYS_OFF_DATA_MODULE) != u64_at(&cblk, 0) {
            return Err("PHYSICS_DATA_MODULE_MISMATCH");
        }
        let position = position_at(&pblk, PHYS_OFF_POSITION);
        if !finite_position(&position) {
            return Err("POSITION_NOT_FINITE");
        }
        Ok(Candidate {
            chr_ptr,
            head,
            cblk,
            container,
            physics,
            position,
            d2: 0.0,
        })
    }

    /// diagnostic only: same chain as probe_candidate but WITHOUT the
    /// `chr_type == NPC` gate, so a boss that carries a different type still
    /// yields a position. Returns (chr_type, npc_id, npc_param, position).
    fn probe_any_type(
        &mut self,
        chr_ptr: usize,
        base: usize,
    ) -> Option<(i32, i32, i32, [f32; 3])> {
        let head = self.backend.read_bytes(chr_ptr, CHR_HEAD_LEN)?;
        if !self.vtable_in_image(base, u64_at(&head, OFF_VTABLE)) {
            return None;
        }
        let container = u64_at(&head, OFF_MODULES);
        if container == 0 {
            return None;
        }
        let cblk = self.backend.read_bytes(container as usize, CONTAINER_LEN)?;
        let physics = u64_at(&cblk, SLOT_PHYSICS_OFF);
        if physics == 0 {
            return None;
        }
        let pblk = self.backend.read_bytes(physics as usize, PHYS_BLOCK_LEN)?;
        if !self.vtable_in_image(base, u64_at(&pblk, PHYS_OFF_VTABLE)) {
            return None;
        }
        if u64_at(&pblk, PHYS_OFF_OWNER) != chr_ptr as u64 {
            return None;
        }
        let position = position_at(&pblk, PHYS_OFF_POSITION);
        if !finite_position(&position) {
            return None;
        }
        Some((
            i32_at(&head, OFF_CHR_TYPE),
            i32_at(&head, OFF_NPC_ID),
            i32_at(&head, OFF_NPC_PARAM_ID),
            position,
        ))
    }

    /// Stage 6: the expensive identity checks, run ONLY for characters that are
    /// already inside the radius. Reuses the byte blocks already read, so the
    /// only extra reads are behavior owner + speed field (+ one page query).
    fn verify_candidate(&mut self, c: &Candidate) -> Result<ActivationSnapshot, &'static str> {
        // npc_param_id is AUXILIARY evidence only. It is recorded in the
        // snapshot and counted in `aux_npc_param_zero`, but it is not a gate -
        // identity rests on the vtable, the owner back-references and the
        // character_id cross-check below.
        let npc_param_id = i32_at(&c.head, OFF_NPC_PARAM_ID);
        let npc_id = i32_at(&c.head, OFF_NPC_ID);
        let character_id = u32_at(&c.head, OFF_CHARACTER_ID);
        if character_id != npc_id as u32 {
            return Err("CHARACTER_ID_CROSSCHECK_FAILED");
        }
        let behavior = u64_at(&c.cblk, SLOT_BEHAVIOR_OFF);
        if behavior == 0 {
            return Err("NULL_BEHAVIOR");
        }
        let owner = self
            .backend
            .read_u64(behavior as usize + BEHAVIOR_OFF_OWNER)
            .ok_or("OWNER_UNREADABLE")?;
        if owner != c.chr_ptr as u64 {
            return Err("OWNER_MISMATCH");
        }
        let addr = behavior
            .checked_add(self.cfg.speed_offset as u64)
            .ok_or("SPEED_ADDR_OVERFLOW")? as usize;
        // The page query is OPT-IN (`SPEEDFORCE_PAGE_CHECK=1`). A failed
        // WriteProcessMemory reports the same condition, and the write result
        // is checked, so on the default path the query is redundant work.
        if self.cfg.page_check && !self.backend.writable(addr) {
            return Err("SPEED_PAGE_NOT_WRITABLE");
        }
        let original = self.backend.read_f32(addr).ok_or("SPEED_UNREADABLE")?;
        if !original.is_finite() || original <= 0.0 {
            return Err("SPEED_INVALID");
        }
        Ok(ActivationSnapshot {
            chr_ptr: c.chr_ptr,
            vtable: u64_at(&c.head, OFF_VTABLE),
            npc_param_id,
            npc_id,
            chr_type: i32_at(&c.head, OFF_CHR_TYPE),
            team_type: c.head[OFF_TEAM_TYPE],
            character_id,
            container: c.container,
            behavior,
            owner,
            physics: c.physics,
            position: c.position,
            distance_m: c.d2.sqrt(),
            speed_addr: addr,
            original,
        })
    }

    /// Reads the three ChrSet arrays and returns deduplicated ChrIns pointers.
    /// Shared by activation and the mid-window rescan.
    fn read_chr_arrays(
        &mut self,
        wcm: usize,
        player: usize,
        report: &mut EnumReport,
    ) -> Vec<usize> {
        let mut candidates: Vec<usize> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let array_bytes = CHR_ARRAY_SLOTS * CHR_ENTRY_SIZE;
        for field in CHR_ARRAY_FIELDS {
            // the WCM fields move while areas stream in. The old code
            // discarded the whole array on the first mismatch, which during
            // streaming silently dropped up to 1500 characters - typically the
            // local enemies - and left the activation targeting only distant
            // NPCs. Retry a few times before giving up on this array.
            let mut accepted = false;
            for attempt in 0..3 {
                let arr = self.backend.read_u64(wcm + field).unwrap_or(0) as usize;
                if arr == 0 {
                    if attempt == 0 {
                        report.arrays_empty += 1;
                    }
                    break;
                }
                // 1500*16 = 24000 bytes exceeds the Backend's 4096-byte
                // single-read ceiling. Read it as six chunks; the slot parsing
                // below is byte-for-byte the P1 logic.
                let raw = match read_array_chunked(&mut self.backend, arr, array_bytes) {
                    Ok(b) => b,
                    Err(e) => {
                        report.arrays_unreadable += 1;
                        self.log(
                            "CHR_ARRAY_UNREADABLE",
                            format!("field={:#x} array={:#x} chunk={} offset={} len={} reason={} note=whole_array_discarded", field, arr, e.chunk_index, e.offset, e.len, e.reason),
                        );
                        break;
                    }
                };
                let arr_after = self.backend.read_u64(wcm + field).unwrap_or(0) as usize;
                if arr_after != arr {
                    report.arrays_base_changed += 1;
                    self.log(
                        "CHR_ARRAY_BASE_CHANGED",
                        format!(
                            "field={:#x} before={:#x} after={:#x} attempt={} note=retrying",
                            field, arr, arr_after, attempt + 1
                        ),
                    );
                    continue;
                }
                report.arrays_read += 1;
                accepted = true;
                for i in 0..CHR_ARRAY_SLOTS {
                    let chr = u64_at(&raw, i * CHR_ENTRY_SIZE) as usize;
                    report.slots_scanned += 1;
                    if chr == 0 {
                        continue;
                    }
                    if chr == player {
                        report.skipped_player += 1;
                        continue;
                    }
                    // The arrays overlap heavily (measured 369 + 194 + 0 unique),
                    // so dedup before the expensive per-target verification.
                    if !seen.insert(chr) {
                        report.duplicates += 1;
                        continue;
                    }
                    candidates.push(chr);
                }
                break;
            }
            if !accepted && report.arrays_base_changed > 0 {
                self.log(
                    "CHR_ARRAY_SKIPPED",
                    format!("field={:#x} note=base_never_stable_after_3_attempts", field),
                );
            }
        }
        candidates
    }

    /// characters that appear AFTER activation (a boss spawning behind a
    /// fog gate, enemies walking into the radius) were never affected, because
    /// activation is a one-shot snapshot. Re-scan on an interval and add only
    /// the new ones; existing targets are untouched so their original value is
    /// never overwritten by the already-slowed reading.
    fn rescan_add_targets(&mut self) {
        let base = self.backend.module_base();
        if base == 0 {
            return;
        }
        let wcm = match self.backend.read_u64(base + self.cfg.wcm_rva) {
            Some(v) if v != 0 => v as usize,
            _ => return,
        };
        let player = self
            .backend
            .read_u64(wcm + self.cfg.player_off)
            .unwrap_or(0) as usize;
        if player == 0 {
            return;
        }
        let ppos = match self.player_position(player, base) {
            Ok(p) => p,
            Err(_) => return,
        };
        let mut rep = EnumReport::default();
        let cands = self.read_chr_arrays(wcm, player, &mut rep);
        let r2 = self.cfg.radius_m * self.cfg.radius_m;
        let cap = if self.cfg.target_limit > 0 {
            self.cfg.target_limit.min(self.cfg.max_targets)
        } else {
            self.cfg.max_targets
        };
        let mut known: std::collections::HashSet<usize> = self
            .targets
            .iter()
            .chain(self.displaced.iter())
            .map(|s| s.chr_ptr)
            .collect();
        let mut added = 0usize;
        let mut writer_fail = 0usize;
        let mut probed = 0usize;
        for chr in cands.iter().take(self.cfg.max_candidates) {
            if self.targets.len() >= cap {
                break;
            }
            if known.contains(chr) {
                continue;
            }
            let mut c = match self.probe_candidate(*chr, player, base) {
                Ok(c) => {
                    probed += 1;
                    c
                }
                Err(_) => continue,
            };
            c.d2 = dist2(&c.position, &ppos);
            if c.d2 > r2 {
                continue;
            }
            let snap = match self.verify_candidate(&c) {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Never write an address we already own: a second write would read
            // the already-slowed value as its "original".
            if self
                .targets
                .iter()
                .chain(self.displaced.iter())
                .any(|t| t.speed_addr == snap.speed_addr)
            {
                continue;
            }
            if self.write_checked(snap.speed_addr, self.cfg.scale) {
                self.record_write(snap.speed_addr, self.cfg.scale);
                if self.undo.len() >= 256 {
                    self.undo.drain(0..128);
                }
                self.undo.push((snap.speed_addr, snap.original));
                self.log(
                    "TARGET_ADDED_LATE",
                    format!(
                        "addr={:#x} chr={:#x} npc_id={} team={} dist_m={:.2} {} -> {}",
                        snap.speed_addr, snap.chr_ptr, snap.npc_id, snap.team_type,
                        snap.distance_m, snap.original, self.cfg.scale
                    ),
                );
                known.insert(*chr);
                self.targets.push(snap);
                added += 1;
            } else {
                writer_fail += 1;
            }
        }
        self.log(
            "RESCAN",
            format!(
                "candidates={} positions_ok={} added={} write_failed={} targets_now={}",
                cands.len(), probed, added, writer_fail, self.targets.len()
            ),
        );
    }

    /// discovery (F10): walk WorldChrMan fields near the known character
    /// array block and report every field that behaves like a chr array, with
    /// the nearest character inside it. In a boss arena this names the field
    /// that actually holds the boss, which the three known arrays do not.
    pub fn probe_chr_fields(&mut self) {
        let base = self.backend.module_base();
        if base == 0 {
            return;
        }
        let wcm = match self.backend.read_u64(base + self.cfg.wcm_rva) {
            Some(v) if v != 0 => v as usize,
            _ => {
                self.log("ARRAY_PROBE", "wcm=0 note=not_in_world");
                return;
            }
        };
        let player = self
            .backend
            .read_u64(wcm + self.cfg.player_off)
            .unwrap_or(0) as usize;
        let ppos = match self.player_position(player, base) {
            Ok(p) => p,
            Err(e) => {
                self.log("ARRAY_PROBE", format!("player_position_failed={}", e));
                return;
            }
        };
        let bytes = CHR_ARRAY_SLOTS * CHR_ENTRY_SIZE;
        let mut found = 0usize;
        let mut off = PROBE_FIELD_LO;
        while off <= PROBE_FIELD_HI {
            let arr = match self.backend.read_u64(wcm + off) {
                Some(v) if v != 0 => v as usize,
                _ => {
                    off += 8;
                    continue;
                }
            };
            let raw = match read_array_chunked(&mut self.backend, arr, bytes) {
                Ok(b) => b,
                Err(_) => {
                    off += 8;
                    continue;
                }
            };
            let mut non_null = 0usize;
            let mut plausible = 0usize;
            let mut best = f32::INFINITY;
            let mut best_desc = String::new();
            for i in 0..CHR_ARRAY_SLOTS {
                let chr = u64_at(&raw, i * CHR_ENTRY_SIZE) as usize;
                if chr == 0 {
                    continue;
                }
                non_null += 1;
                let vt = match self.backend.read_u64(chr) {
                    Some(v) => v,
                    None => continue,
                };
                if !self.vtable_in_image(base, vt) {
                    continue;
                }
                plausible += 1;
                if let Some((ct, id, param, pos)) = self.probe_any_type(chr, base) {
                    let d = dist2(&pos, &ppos).sqrt();
                    if d < best {
                        best = d;
                        best_desc = format!(
                            "{:.1}m/ct{}/npc{}/p{}", d, ct, id, param
                        );
                    }
                }
            }
            if plausible > 0 {
                found += 1;
                self.log(
                    "ARRAY_PROBE",
                    format!(
                        "field={:#x} arr={:#x} entries={} plausible={} nearest={}",
                        off, arr, non_null, plausible, best_desc
                    ),
                );
            }
            off += 8;
        }
        self.log("ARRAY_PROBE_DONE", format!("fields_with_chrs={}", found));
    }

    /// One activation = one bounded scan. Idle never scans.
    fn select_targets(&mut self) -> Result<Vec<ActivationSnapshot>, &'static str> {        let base = self.backend.module_base();
        if base == 0 {
            return Err("MODULE_BASE_UNAVAILABLE");
        }

        // ---- stage 2: WorldChrMan, version gate, character arrays --------
        let t_enum = std::time::Instant::now();
        let slot = base.checked_add(self.cfg.wcm_rva).ok_or("WCM_RVA_OVERFLOW")?;
        let wcm = self.backend.read_u64(slot).ok_or("WCM_SLOT_UNREADABLE")? as usize;
        if wcm == 0 {
            return Err("WCM_NULL");
        }

        // Version gate. The vtable of CS::WorldChrManImp is a compile-time
        // constant of the build these offsets came from. If it does not match,
        // every offset below describes a different binary - reading them would
        // produce confident nonsense, which is exactly how the 1.16.1 build
        // behaved on 1.17.2 ("nothing happens, no error"). Refuse loudly.
        let wcm_vtable = self.backend.read_u64(wcm).ok_or("WCM_VTABLE_UNREADABLE")? as usize;
        let expected_vtable = base.wrapping_add(WCM_VTABLE_RVA);
        if wcm_vtable != expected_vtable {
            self.log(
                "GAME_VERSION_MISMATCH",
                format!(
                    "expected_vtable={:#x} found={:#x} wcm={:#x} \
                     note=exe is not the build these offsets were derived from, \
                     refusing all further work",
                    expected_vtable, wcm_vtable, wcm
                ),
            );
            return Err("GAME_VERSION_MISMATCH");
        }

        // The player is excluded by ChrIns POINTER, and only by pointer. If the
        // pointer cannot be resolved the activation is refused instead of
        // guessed: slowing the player is the one failure this design must never
        // produce. Team is not consulted - friendly and neutral characters are
        // valid targets like any other.
        let mut player = self
            .backend
            .read_u64(wcm + self.cfg.player_off)
            .unwrap_or(0) as usize;
        if player == 0 {
            player = self.cfg.player_chr_ptr;
        }
        if player == 0 {
            return Err("PLAYER_UNKNOWN");
        }

        // right after an area load the character arrays can still be a
        // stale generation (thousands of dead pointers, the local enemies not
        // in it yet) and the player position can lag behind. A scan that lands
        // almost nothing in range is therefore retried BEFORE any write, and
        // the attempt with the most in-range candidates wins.
        let r2 = self.cfg.radius_m * self.cfg.radius_m;
        let mut report = EnumReport::default();
        let mut inside: Vec<Candidate> = Vec::new();
        let mut player_pos = [0.0f32; 3];
        self.begin_phase();
        let t_pos = std::time::Instant::now();
        for attempt in 0..SCAN_ATTEMPTS {
            let mut rep = EnumReport::default();
            let candidates = self.read_chr_arrays(wcm, player, &mut rep);
            rep.candidates = candidates.len();
            if rep.arrays_read < CHR_ARRAY_FIELDS.len() {
                self.log(
                    "SCAN_INCOMPLETE",
                    format!(
                        "arrays_read={} of {} base_changed={} unreadable={} attempt={} \
                         note=result_may_miss_local_enemies",
                        rep.arrays_read, CHR_ARRAY_FIELDS.len(),
                        rep.arrays_base_changed, rep.arrays_unreadable, attempt + 1
                    ),
                );
            }
            if rep.arrays_read == 0 {
                // Not fatal on its own: an empty world (menu / loading) looks
                // like this, and the caller treats it as retryable.
                return Err("NO_CHR_ARRAYS");
            }
            if candidates.len() > self.cfg.max_candidates {
                rep.dropped_scan = candidates.len() - self.cfg.max_candidates;
                self.log(
                    "SCAN_TRUNCATED",
                    format!(
                        "candidates={} scanned={} dropped_scan={}",
                        candidates.len(), self.cfg.max_candidates, rep.dropped_scan
                    ),
                );
            }
            let ppos = self.player_position(player, base)?;
            let mut attempt_inside: Vec<Candidate> = Vec::new();
            for chr in candidates.iter().take(self.cfg.max_candidates) {
                match self.probe_candidate(*chr, player, base) {
                    Ok(mut c) => {
                        rep.positions_read += 1;
                        bump_type(&mut rep.type_counts, i32_at(&c.head, OFF_CHR_TYPE));
                        c.d2 = dist2(&c.position, &ppos);
                        let d = c.d2.sqrt();
                        rep.nearest.push((
                            d,
                            i32_at(&c.head, OFF_NPC_ID),
                            c.head[OFF_TEAM_TYPE],
                        ));
                        let b = if d <= 5.0 { 0 }
                            else if d <= 10.0 { 1 }
                            else if d <= 25.0 { 2 }
                            else if d <= 50.0 { 3 }
                            else if d <= 100.0 { 4 }
                            else { 5 };
                        rep.buckets[b] += 1;
                        rep.nearest_pos.push((c.position, d));
                        if c.d2 <= r2 {
                            rep.inside_radius += 1;
                            attempt_inside.push(c);
                        } else {
                            rep.outside_radius += 1;
                        }
                    }
                    Err(reason) => {
                        rep.skipped_invalid += 1;
                        if rep.skip_reasons.len() < 8 {
                            rep.skip_reasons.push(reason);
                        }
                        match rep.reason_counts.iter_mut().find(|(r, _)| *r == reason) {
                            Some(e) => e.1 += 1,
                            None => rep.reason_counts.push((reason, 1)),
                        }
                        // keep the nearest characters that only failed
                        // the chr_type screen - a boss with another type lands
                        // here instead of vanishing without a trace.
                        if reason == "NOT_NPC" {
                            if let Some((ct, id, param, pos)) = self.probe_any_type(*chr, base) {
                                let d = dist2(&pos, &ppos).sqrt();
                                if rep.non_npc_near.len() < 16 {
                                    rep.non_npc_near.push((d, ct, id, param, *chr));
                                }
                            }
                        }
                        // chr_type census: ONLY for pointers whose vtable is
                        // inside the game image. Without that screen the census
                        // reads random heap bytes and prints nonsense types.
                        if reason != "CHRINS_UNREADABLE" {
                            let plausible = self
                                .backend
                                .read_u64(*chr)
                                .map(|vt| self.vtable_in_image(base, vt))
                                .unwrap_or(false);
                            if plausible {
                                if let Some(b) = self.backend.read_bytes(*chr + OFF_CHR_TYPE, 4) {
                                    let ct = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                                    bump_type(&mut rep.type_counts, ct);
                                }
                            }
                        }
                    }
                }
            }
            let got = attempt_inside.len();
            let readable = rep.positions_read;
            let cands_n = rep.candidates;
            // Degenerate: nothing (or almost nothing) in range, or the arrays
            // are mostly dead pointers. Both mean "the world is not settled".
            let degenerate = got <= 3 || readable.saturating_mul(3) < cands_n;
            if got >= inside.len() {
                inside = attempt_inside;
                player_pos = ppos;
                report = rep;
            }
            if !degenerate {
                break;
            }
            if attempt + 1 < SCAN_ATTEMPTS {
                self.log(
                    "SCAN_RETRY",
                    format!(
                        "attempt={} inside={} candidates={} readable={} \
                         note=world_not_settled_yet",
                        attempt + 1, got, cands_n, readable
                    ),
                );
                std::thread::sleep(std::time::Duration::from_millis(SCAN_RETRY_MS));
            }
        }
        self.timing.enum_us = t_enum.elapsed().as_micros() as u64;
        self.timing.position_us = t_pos.elapsed().as_micros() as u64;
        self.log_perf("scan", t_pos.elapsed().as_nanos() as u64);

        // ---- stage 6: nearest first, then verify -> write PER TARGET ------
        // Ordering before verification preserves "nearest valid target first"
        // while avoiding BehaviorModule work for targets we would drop anyway.
        // The write happens immediately after that target's own verification, so
        // a mid-way failure only ever has to undo what was really written.
        inside.sort_by(|a, b| a.d2.partial_cmp(&b.d2).unwrap_or(std::cmp::Ordering::Equal));
        // TARGET_LIMIT=0 means "no extra limit"; MAX_TARGETS (<=128) still caps
        // the work, so an unbounded number of writes is impossible by design.
        let cap = if self.cfg.target_limit > 0 {
            self.cfg.target_limit.min(self.cfg.max_targets)
        } else {
            self.cfg.max_targets
        };
        report.cap = cap;
        self.begin_phase();
        let t_ver = std::time::Instant::now();
        let mut out: Vec<ActivationSnapshot> = Vec::new();
        // the character arrays dedupe on ChrIns*, but two distinct
        // ChrIns can resolve to the same BehaviorModule / speed address. If we
        // wrote twice, the second verify would read the ALREADY-SLOWED value
        // as `original` and restoring would leave the field at 0.2 forever.
        // Dedupe on the write address itself, before any write takes place.
        let mut seen_speed_addrs: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut write_us: u64 = 0;
        let mut write_failed = false;
        for (idx, c) in inside.iter().enumerate() {
            if out.len() >= cap {
                report.dropped_cap = inside.len() - idx;
                self.log(
                    "CAP_DROPPED",
                    format!(
                        "cap={} inside_radius={} dropped_cap={} note=left_untouched",
                        cap, report.inside_radius, report.dropped_cap
                    ),
                );
                break;
            }
            let snap = match self.verify_candidate(c) {
                Ok(s) => s,
                Err(reason) => {
                    report.skipped_invalid += 1;
                    if report.skip_reasons.len() < 8 {
                        report.skip_reasons.push(reason);
                    }
                    continue;
                }
            };
            if !claim_speed_addr(&mut seen_speed_addrs, snap.speed_addr) {
                // Dedupe BEFORE the write. Logged once after the loop (below)
                // so the evidence survives release/LTO dead-branch removal.
                report.duplicate_speed_addrs += 1;
                continue;
            }
            report.verified += 1;
            if snap.npc_param_id <= 0 {
                // Auxiliary evidence only. Identity is carried by the vtable,
                // the owner back-references and the character_id cross-check, so
                // a zero here is recorded but never used to veto a write.
                report.aux_npc_param_zero += 1;
                self.log(
                    "NPC_PARAM_ID_ZERO_AUX",
                    format!("chr={:#x} addr={:#x} aux_only=true proceeding",
                            snap.chr_ptr, snap.speed_addr),
                );
            }
            let t_w = std::time::Instant::now();
            let wrote = self.write_checked(snap.speed_addr, self.cfg.scale);
            write_us += t_w.elapsed().as_micros() as u64;
            if !wrote {
                write_failed = true;
                break;
            }
            self.record_write(snap.speed_addr, self.cfg.scale);
            // remember the original so the F9 sweep can undo a write
            // whose target was lost later in the window.
            if self.undo.len() >= 256 {
                self.undo.drain(0..128);
            }
            self.undo.push((snap.speed_addr, snap.original));
            self.log(
                "TARGET_WRITTEN",
                format!("addr={:#x} chr={:#x} npc_id={} npc_param={} chr_type={} team={} \
                         dist_m={:.2} {} -> {}",
                        snap.speed_addr, snap.chr_ptr, snap.npc_id,
                        snap.npc_param_id, snap.chr_type, snap.team_type,
                        snap.distance_m, snap.original, self.cfg.scale),
            );
            if snap.chr_type != CHR_TYPE_NPC {
                self.log(
                    "BOSS_TYPE_TARGET",
                    format!(
                        "addr={:#x} npc_id={} npc_param={} chr_type={} dist_m={:.2} note=boss_type_accepted",
                        snap.speed_addr, snap.npc_id, snap.npc_param_id,
                        snap.chr_type, snap.distance_m
                    ),
                );
            }
            out.push(snap);
        }
        if report.duplicate_speed_addrs > 0 {
            self.log(
                "DUPLICATE_SPEED_ADDR",
                format!(
                    "count={} note=same_speed_field_as_an_earlier_target_write_skipped \
                     no_second_write=1 original_preserved=1",
                    report.duplicate_speed_addrs
                ),
            );
        }
        let ver_ns = t_ver.elapsed().as_nanos() as u64;
        self.timing.verify_us = ver_ns / 1000;
        self.timing.write_us = write_us;
        self.log_perf("verify", ver_ns);
        self.log(
            "WRITE_SUMMARY",
            format!("write_us={} written={} verified={} per_target_us={}",
                    write_us, out.len(), report.verified,
                    if out.is_empty() { 0 } else { write_us / out.len() as u64 }),
        );
        report.written = out.len();
        let dists: Vec<String> = out.iter().map(|s| format!("{:.1}", s.distance_m)).collect();
        self.log(
            "SELECT_SUMMARY",
            format!(
                "radius_m={} slots={} arrays={} arrays_empty={} arrays_unreadable={} \
                 dupes={} dup_speed={} base_changed={} player_skipped={} candidates={} \
                 dropped_scan={} positions_read={} inside={} outside={} verified={} written={} \
                 cap={} dropped_cap={} limit={} aux_npc_param_zero={} player=({:.2},{:.2},{:.2}) \
                 dists=[{}] reasons={:?}",
                self.cfg.radius_m, report.slots_scanned, report.arrays_read,
                report.arrays_empty, report.arrays_unreadable,
                report.duplicates, report.duplicate_speed_addrs,
                report.arrays_base_changed,
                report.skipped_player, report.candidates, report.dropped_scan,
                report.positions_read, report.inside_radius, report.outside_radius,
                report.verified, report.written, report.cap, report.dropped_cap,
                self.cfg.target_limit, report.aux_npc_param_zero,
                player_pos[0], player_pos[1], player_pos[2],
                dists.join(","), report.skip_reasons
            ),
        );
        // diagnostics: who is actually around, and why the rest was
        // dropped. Written once per activation, cheap, and it is the only way
        // to tell "no enemies nearby" from "enemies filtered out".
        report.nearest.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        report.nearest_pos.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        let near: Vec<String> = report
            .nearest
            .iter()
            .take(12)
            .map(|(d, id, team)| format!("{:.1}m/npc{}/t{}", d, id, team))
            .collect();
        let raw: Vec<String> = report
            .nearest_pos
            .iter()
            .take(5)
            .map(|(p, d)| format!("{:.1}m@({:.1},{:.1},{:.1})", d, p[0], p[1], p[2]))
            .collect();
        self.log(
            "SCAN_NEAREST",
            format!(
                "{} | buckets 0-5={} 5-10={} 10-25={} 25-50={} 50-100={} >100={} \
                 | player=({:.2},{:.2},{:.2}) ptr={:#x} | raw {}",
                near.join(" "),
                report.buckets[0], report.buckets[1], report.buckets[2],
                report.buckets[3], report.buckets[4], report.buckets[5],
                player_pos[0], player_pos[1], player_pos[2], player,
                raw.join(" ")
            ),
        );
        let reasons: Vec<String> = report
            .reason_counts
            .iter()
            .map(|(r, n)| format!("{}={}", r, n))
            .collect();
        self.log(
            "SCAN_REASONS",
            format!("total={} {}", report.skipped_invalid, reasons.join(" ")),
        );
        // chr_type census, descending: `ct5=446` is the normal NPC bucket, any
        // other bucket that appears while a boss is on screen names its type.
        report.type_counts.sort_by(|a, b| b.1.cmp(&a.1));
        let types: Vec<String> = report
            .type_counts
            .iter()
            .take(8)
            .map(|(ct, n)| format!("ct{}={}", ct, n))
            .collect();
        if !types.is_empty() {
            self.log("CHR_TYPE_HIST", types.join(" "));
        }
        if !report.non_npc_near.is_empty() {
            report
                .non_npc_near
                .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let list: Vec<String> = report
                .non_npc_near
                .iter()
                .take(6)
                .map(|(d, ct, id, param, chr)| {
                    format!("{:.1}m/ct{}/npc{}/p{}/chr{:#x}", d, ct, id, param, chr)
                })
                .collect();
            self.log(
                "NON_NPC_NEAR",
                format!("count={} {}", report.non_npc_near.len(), list.join(" ")),
            );
        }
        self.last_enum = Some(report);
        if write_failed {
            // Atomicity: never leave a half-slowed group behind.
            let (reverted, revert_failed) = self.rollback_partial(&out);
            if revert_failed > 0 {
                self.log(
                    "ROLLBACK_INCOMPLETE",
                    format!("reverted={}/{} failed={} -> restart game",
                            reverted, out.len(), revert_failed),
                );
                return Err("ACTIVATION_WRITE_FAILED_ROLLBACK_INCOMPLETE__RESTART_GAME");
            }
            self.log("ROLLBACK", format!("{}/{} reverted", reverted, out.len()));
            return Err("ACTIVATION_WRITE_FAILED_ROLLED_BACK");
        }
        if out.is_empty() {
            return Err("NO_VALID_TARGET_IN_CHRSET");
        }
        Ok(out)
    }

    /// Undo a partially applied activation, newest write first. Returns
    /// `(reverted, still_broken)`; the caller decides whether that is a clean
    /// rollback or a restart-the-game state.
    fn rollback_partial(&mut self, applied: &[ActivationSnapshot]) -> (usize, usize) {
        let mut reverted = 0usize;
        let mut failed = 0usize;
        for w in applied.iter().rev() {
            if self.write_checked(w.speed_addr, w.original) {
                self.record_write(w.speed_addr, w.original);
                reverted += 1;
            } else {
                failed += 1;
                self.log(
                    "ROLLBACK_WRITE_FAILED",
                    format!("addr={:#x} still at {}", w.speed_addr, self.cfg.scale),
                );
            }
        }
        (reverted, failed)
    }

    fn classify_one(&mut self, s: &ActivationSnapshot) -> ObjectState {
        let p = s.chr_ptr;
        let head = match self.backend.read_bytes(p, CHR_HEAD_LEN) {
            Some(b) => b,
            None => return ObjectState::Unknown("CHRINS_UNREADABLE"),
        };
        macro_rules! same {
            ($got:expr, $want:expr, $why:expr) => {
                if $got != $want {
                    return ObjectState::Unknown($why);
                }
            };
        }
        same!(u64_at(&head, OFF_VTABLE), s.vtable, "VTABLE_CHANGED");
        same!(i32_at(&head, OFF_NPC_PARAM_ID), s.npc_param_id, "NPC_PARAM_ID_CHANGED");
        same!(i32_at(&head, OFF_NPC_ID), s.npc_id, "NPC_ID_CHANGED");
        same!(i32_at(&head, OFF_CHR_TYPE), s.chr_type, "CHR_TYPE_CHANGED");
        same!(head[OFF_TEAM_TYPE], s.team_type, "TEAM_CHANGED");
        same!(u32_at(&head, OFF_CHARACTER_ID), s.character_id, "CHARACTER_ID_CHANGED");
        same!(u64_at(&head, OFF_MODULES), s.container, "CONTAINER_CHANGED");
        match self.backend.read_u64(s.container as usize + SLOT_BEHAVIOR_OFF) {
            Some(v) if v == s.behavior => {}
            _ => return ObjectState::Unknown("BEHAVIOR_CHANGED"),
        }
        match self.backend.read_u64(s.behavior as usize + BEHAVIOR_OFF_OWNER) {
            Some(v) if v == s.owner => {}
            _ => return ObjectState::Unknown("OWNER_CHANGED"),
        }
        match self.backend.read_f32(s.speed_addr) {
            Some(v) if v.is_finite() && v > 0.0 => {
                // only a field that still holds OUR scale may be
                // written back. Anything else was changed by the game or by
                // another mod during the window; restoring `original` on top
                // of it would destroy that third party's state.
                if v.to_bits() == self.cfg.scale.to_bits() {
                    ObjectState::SameObject
                } else {
                    ObjectState::ExternalModified(v)
                }
            }
            _ => ObjectState::ControlFailed("SPEED_VALUE_UNREADABLE"),
        }
    }

    /// last-resort undo for a target whose cached address went stale.
    ///
    /// `speed_addr` is `behavior + speed_offset`, and `behavior` is re-derived
    /// from the chr on every activation, so a rebuilt physics/behavior module
    /// leaves a stale address while the character itself is unchanged. Re-walk
    /// the chain from `chr_ptr`; require chr-level identity (vtable, type,
    /// ids, team) and the behavior -> chr owner back-reference; then write the
    /// original back only if the field still holds exactly our scale, which
    /// proves it is the value we wrote and not somebody else's.
    fn re_resolve_restore(&mut self, s: &ActivationSnapshot, reason: &'static str) -> ReresolveOutcome {
        let head = match self.backend.read_bytes(s.chr_ptr, CHR_HEAD_LEN) {
            Some(b) => b,
            None => return ReresolveOutcome::Abandoned("CHRINS_UNREADABLE"),
        };
        if u64_at(&head, OFF_VTABLE) != s.vtable
            || i32_at(&head, OFF_CHR_TYPE) != s.chr_type
            || i32_at(&head, OFF_NPC_ID) != s.npc_id
            || u32_at(&head, OFF_CHARACTER_ID) != s.character_id
            || head[OFF_TEAM_TYPE] != s.team_type
        {
            return ReresolveOutcome::Abandoned("CHR_IDENTITY_MISMATCH");
        }
        let container = u64_at(&head, OFF_MODULES);
        if container == 0 {
            return ReresolveOutcome::Abandoned("CONTAINER_NULL");
        }
        let behavior = match self.backend.read_u64(container as usize + SLOT_BEHAVIOR_OFF) {
            Some(v) if v != 0 => v,
            _ => return ReresolveOutcome::Abandoned("BEHAVIOR_UNREADABLE"),
        };
        match self.backend.read_u64(behavior as usize + BEHAVIOR_OFF_OWNER) {
            Some(v) if v == s.chr_ptr as u64 => {}
            _ => return ReresolveOutcome::Abandoned("OWNER_MISMATCH"),
        }
        let addr = behavior.wrapping_add(self.cfg.speed_offset as u64) as usize;
        let cur = match self.backend.read_f32(addr) {
            Some(v) if v.is_finite() => v,
            _ => return ReresolveOutcome::Abandoned("SPEED_UNREADABLE"),
        };
        if cur.to_bits() == s.original.to_bits() {
            // The game already normalised it; writing would be a no-op with
            // extra risk.
            self.log(
                "TARGET_ALREADY_NORMAL",
                format!("addr={:#x} value={} ({})", addr, cur, reason),
            );
            return ReresolveOutcome::AlreadyNormal(addr);
        }
        if cur.to_bits() != self.cfg.scale.to_bits() {
            return ReresolveOutcome::Abandoned("FIELD_OWNED_BY_OTHER");
        }
        if self.write_checked(addr, s.original) {
            self.record_write(addr, s.original);
            self.log(
                "TARGET_RESTORED_RERESOLVED",
                format!(
                    "old_addr={:#x} new_addr={:#x} -> {} (container rebuilt, {})",
                    s.speed_addr, addr, s.original, reason
                ),
            );
            ReresolveOutcome::Restored(addr)
        } else {
            self.log(
                "TARGET_RERESOLVE_WRITE_FAILED",
                format!("addr={:#x} ({})", addr, reason),
            );
            ReresolveOutcome::Failed(addr)
        }
    }

    fn probe_hold(&mut self, tag: &'static str) {
        let n = self.cfg.hold_probe_targets.min(self.targets.len());
        for i in 0..n {
            let snap = self.targets[i].clone();
            let value = self.backend.read_f32(snap.speed_addr);
            let at = self.backend.now_ms();
            self.holds.push(HoldProbe { tag, addr: snap.speed_addr, at_ms: at, value });
            self.log(
                "SPEED_HOLD",
                format!("tag={} addr={:#x} value={:?} expect={}", tag, snap.speed_addr,
                        value, self.cfg.scale),
            );
        }
    }

    /// The single entry point for every trigger.
    ///
    /// Rules (identical for all sources):
    ///   * only Idle may start;
    ///   * Active rejects, with the source recorded, so a pill used mid-flight
    ///     cannot extend or restart the window;
    ///   * Halted rejects and says why (a halted controller must not be revived
    ///     by a trigger);
    ///   * the caller is responsible for edge detection - this function is
    ///     level-triggered on purpose, so a stuck signal cannot re-fire it.
    pub fn request_activation(&mut self, src: TriggerSource, now: u64) -> bool {
        match self.state.clone() {
            State::Halted(r) => {
                self.last_refusal = Some(format!("HALTED:{}", r));
                self.log(
                    "TRIGGER_REJECTED",
                    format!("source={} reason=HALTED:{}", src.as_str(), r),
                );
                return false;
            }
            State::Active => {
                // A pill used during the 15 s window is refused. NOTE: the
                // game has already consumed the item by the time this signal
                // arrives; refusing here does NOT return it. That is a known,
                // still-open design issue - do not claim otherwise.
                self.last_refusal = Some("ALREADY_ACTIVE".into());
                self.log(
                    "TRIGGER_REJECTED",
                    format!("source={} reason=ALREADY_ACTIVE", src.as_str()),
                );
                return false;
            }
            State::Idle => {}
        }
        self.last_refusal = None;
        self.log("TRIGGER_ACCEPTED", format!("source={}", src.as_str()));
        self.activate(now);
        let started = self.state == State::Active;
        if !started && self.last_refusal.is_none() {
            // activate() refused via its retryable path (e.g. empty world).
            // It must still set a reason; this is a safety net if a new path
            // forgets to.
            self.last_refusal = Some("CORE_DID_NOT_START".into());
        }
        started
    }

    fn activate(&mut self, tick_now: u64) {
        self.cfg.player_chr_ptr = self.backend.target_ptr();

        let t_scan = std::time::Instant::now();
        let targets = match self.select_targets() {
            Ok(t) => t,
            Err(r) => {
                self.log("ACTIVATION_REFUSED", r);
                self.last_refusal = Some(r.to_string());
                if r == "NO_VALID_TARGET_IN_CHRSET"
                    || r == "NO_CHR_ARRAYS"
                    || r.starts_with("PLAYER_")
                {
                    // Retryable (menu / area still streaming). Stay Idle.
                    return;
                }
                self.halt(r);
                return;
            }
        };
        self.timing.total_us = t_scan.elapsed().as_micros() as u64;

        // `select_targets` already wrote every target (verify -> write),
        // so there is nothing left to apply here and no second pass to fail in.
        self.targets = targets;
        self.displaced.clear();
        self.probe_flags = [false; 2];
        // TIMER FIX: the 15 s budget starts AFTER every write has completed.
        self.started_ms = self.backend.now_ms();
        self.last_rescan_ms = self.started_ms;
        self.timing.activation_span_ms = self.started_ms.saturating_sub(tick_now);
        self.state = State::Active;
        let nearest = self.targets.iter()
            .map(|t| t.distance_m).fold(f32::INFINITY, f32::min);
        let farthest = self.targets.iter()
            .map(|t| t.distance_m).fold(0.0f32, f32::max);
        let rep = self.last_enum.clone().unwrap_or_default();
        self.log(
            "ACTIVATED",
            format!("targets={} radius_m={} nearest_m={:.2} farthest_m={:.2} \
                     duration_ms={} prep_ms={} inside_radius={} verified={} written={} \
                     cap={} dropped_cap={} slots={} dupes={} dropped_scan={}",
                    self.targets.len(), self.cfg.radius_m, nearest, farthest,
                    self.cfg.duration_ms, self.timing.activation_span_ms,
                    rep.inside_radius, rep.verified, rep.written, rep.cap,
                    rep.dropped_cap, rep.slots_scanned, rep.duplicates, rep.dropped_scan),
        );
        self.log(
            "TIMING",
            format!("enum_us={} position_us={} verify_us={} scan_total_us={} write_us={} \
                     prep_ms={} targets={}",
                    self.timing.enum_us, self.timing.position_us,
                    self.timing.verify_us, self.timing.total_us,
                    self.timing.write_us, self.timing.activation_span_ms,
                    self.targets.len()),
        );
        self.probe_hold("immediate");
    }

    fn restore_all(&mut self, reason: &'static str) -> RestoreOutcome {
        if self.state != State::Active {
            return RestoreOutcome::NotActive;
        }
        self.begin_phase();
        let t = std::time::Instant::now();
        self.probe_hold("pre_restore");
        let mut snaps = std::mem::take(&mut self.targets);
        // forfeited targets come back into the restore pass, because the
        // chain can be re-derived from chr_ptr even when the cached address is
        // stale. Without this a displaced boss kept its slow value forever.
        let displaced = std::mem::take(&mut self.displaced);
        snaps.extend(displaced);
        let mut restored = 0usize;
        let mut reresolved = 0usize;
        let mut forbidden = 0usize;
        let mut failed = 0usize;
        let mut external_skipped = 0usize;
        for snap in &snaps {
            // Only SameObject may be written back.
            match self.classify_one(snap) {
                ObjectState::SameObject => {
                    if self.write_checked(snap.speed_addr, snap.original) {
                        self.record_write(snap.speed_addr, snap.original);
                        restored += 1;
                        self.log("TARGET_RESTORED",
                                 format!("addr={:#x} -> {} ({})", snap.speed_addr,
                                         snap.original, reason));
                    } else {
                        failed += 1;
                        self.log("TARGET_RESTORE_FAILED",
                                 format!("addr={:#x} ({})", snap.speed_addr, reason));
                    }
                }
                ObjectState::ControlFailed(r) => {
                    failed += 1;
                    self.log("TARGET_CONTROL_FAILED_NO_RESTORE",
                             format!("addr={:#x} reason={} write_skipped=true requires_game_restart=true",
                                     snap.speed_addr, r));
                }
                ObjectState::Unknown(r) => {
                    // The cached address went stale. Try to re-derive it from
                    // the character itself before giving up on this target.
                    match self.re_resolve_restore(snap, reason) {
                        ReresolveOutcome::Restored(_) => {
                            restored += 1;
                            reresolved += 1;
                        }
                        ReresolveOutcome::AlreadyNormal(_) => {
                            reresolved += 1;
                        }
                        ReresolveOutcome::Failed(_) => {
                            failed += 1;
                        }
                        ReresolveOutcome::Abandoned(r2) => {
                            forbidden += 1;
                            self.log("TARGET_NO_RESTORE_UNKNOWN",
                                     format!("addr={:#x} reason={} reresolve={} write_forbidden=true",
                                             snap.speed_addr, r, r2));
                        }
                    }
                }
                ObjectState::ExternalModified(v) => {
                    // Deliberately NOT a failure and NOT a halt: somebody else
                    // owns this value now. Leave it exactly as we found it.
                    external_skipped += 1;
                    self.log(
                        "TARGET_NO_RESTORE_EXTERNAL",
                        format!(
                            "addr={:#x} observed={} our_scale={} original={} \
                             write_skipped=true reason=field_changed_externally",
                            snap.speed_addr, v, self.cfg.scale, snap.original
                        ),
                    );
                }
            }
        }
        let restore_ns = t.elapsed().as_nanos() as u64;
        self.timing.restore_us = restore_ns / 1000;
        self.log_perf("restore", restore_ns);
        self.log("RESTORE_SUMMARY",
                 format!("restored={} reresolved={} forbidden={} failed={} \
                          external_skipped={} reason={} restore_us={}",
                         restored, reresolved, forbidden, failed,
                         external_skipped, reason, self.timing.restore_us));
        if forbidden > 0 {
            // Objects we cannot prove identical are left untouched (writing
            // into a reused slot is worse than a stale scale). This is a
            // per-target outcome, NOT a controller failure: halting here made
            // one corpse despawn kill the whole feature for the rest of the
            // session. Log it and return to Idle so the next trigger works.
            self.log(
                "FORBIDDEN_TARGETS_LEFT_ALONE",
                format!("count={} requires_game_restart=false", forbidden),
            );
        }
        if failed > 0 {
            self.halt("RESTORE_FAILED__RESTART_GAME");
            return RestoreOutcome::RestoreFailed;
        }
        self.state = State::Idle;
        if reason == "SHUTDOWN" {
            self.log("SHUTDOWN_RESTORED", format!("{} target(s)", restored));
        }
        RestoreOutcome::Restored
    }

    pub fn shutdown_restore(&mut self) -> RestoreOutcome {
        self.restore_all("SHUTDOWN")
    }

    pub fn restore_now(&mut self) -> bool {
        matches!(self.shutdown_restore(),
                 RestoreOutcome::Restored | RestoreOutcome::NotActive)
    }

    pub fn requires_game_restart(&self) -> bool {
        matches!(self.state, State::Halted(r) if r.ends_with("__RESTART_GAME"))
    }

    pub fn tick(&mut self) {
        let now = self.backend.now_ms();
        let key = self.backend.key_down();
        let edge = key && !self.prev_key;
        self.prev_key = key;
        // F9 = manual sweep, works in any state (including Halted), so a
        // target that went missing can always be undone without a restart.
        let skey = self.backend.sweep_key();
        if skey && !self.prev_sweep {
            self.force_restore_sweep();
        }
        self.prev_sweep = skey;
        // F10 dumps every candidate chr array - the discovery tool for a
        // container that the three known arrays do not cover (bosses).
        let pkey = self.backend.probe_key();
        if pkey && !self.prev_probe {
            self.probe_chr_fields();
        }
        self.prev_probe = pkey;
        self.tick_count = self.tick_count.wrapping_add(1);

        match self.state.clone() {
            State::Halted(_) => return,
            State::Idle => {
                if edge {
                    if self.cfg.hotkey_enabled {
                        self.request_activation(TriggerSource::DebugHotkey, now);
                    } else if !self.hotkey_disabled_logged {
                        // Log once, not every press: the point is to make the
                        // INI switch visible, not to spam the log.
                        self.hotkey_disabled_logged = true;
                        self.log(
                            "HOTKEY_DISABLED",
                            "DebugHotkeyEnabled=0 -> F8 ignored (item/skill triggers are the \
                             intended entry points)",
                        );
                    }
                }
            }
            State::Active => {
                let elapsed = now.saturating_sub(self.started_ms);
                // F8 is START-ONLY. A press while Active is ignored so a
                // 15 s window can never be cut short by an accidental keypress.
                // The window is deliberately GLOBAL: whatever was adopted during
                // it ends together, so the effective duration always matches the
                // configured one. Per-target deadlines were tried and rejected -
                // they made "how long does this last" unpredictable.
                if elapsed >= self.cfg.duration_ms {
                    self.restore_all("AUTO_TIMEOUT");
                    return;
                }
                // adopt characters that appeared after activation (a
                // boss spawning behind a fog gate, enemies streaming in).
                // Right after the trigger the world is usually still settling,
                // so poll fast for the first few seconds, then relax.
                let interval = if elapsed < 15000 { 1000 } else { RESCAN_MS };
                if now.saturating_sub(self.last_rescan_ms) >= interval {
                    self.last_rescan_ms = now;
                    self.rescan_add_targets();
                }
                for (i, due) in HOLD_PROBE_MS.iter().enumerate() {
                    if !self.probe_flags[i] && elapsed >= *due {
                        self.probe_flags[i] = true;
                        self.probe_hold(if i == 0 { "t500" } else { "t2000" });
                    }
                }
                // Rotate ONE target per tick: full coverage every N*16 ms,
                // instead of re-reading every target every 5 ticks.
                let t = std::time::Instant::now();
                if !self.targets.is_empty() {
                    let idx = (self.tick_count as usize) % self.targets.len();
                    let snap = self.targets[idx].clone();
                    if let ObjectState::Unknown(r) = self.classify_one(&snap) {
                        // a single target losing its identity (corpse
                        // despawn, slot reuse, module swap) used to halt the
                        // WHOLE session via restore_all("IDENTITY_LOST"). With
                        // all-radius selection that fires almost every run, so
                        // the mod looked "sometimes works, sometimes dead".
                        // Re-check once to reject transient reads, then drop
                        // just this target and keep the effect alive.
                        if let ObjectState::Unknown(r2) = self.classify_one(&snap) {
                            let left = self.targets.len().saturating_sub(1);
                            self.log(
                                "TARGET_FORFEITED",
                                format!(
                                    "addr={:#x} reason={} remaining={} \
                                     effect_continues=true write_forbidden=true \
                                     deferred_reresolve=true",
                                    snap.speed_addr, r2, left
                                ),
                            );
                            // Keep the snapshot: at restore time the chain can
                            // be re-derived from chr_ptr, so the write can still
                            // be undone instead of leaving a stale slow value.
                            self.displaced.push(snap.clone());
                            self.targets.remove(idx);
                            if self.targets.is_empty() {
                                self.log(
                                    "ALL_TARGETS_FORFEITED",
                                    "no verified target left; ending window early",
                                );
                                self.restore_all("ALL_TARGETS_FORFEITED");
                                return;
                            }
                        } else {
                            self.log(
                                "IDENTITY_TRANSIENT",
                                format!("addr={:#x} reason={} recheck=ok", snap.speed_addr, r),
                            );
                        }
                    }
                }
                self.timing.periodic_us += t.elapsed().as_micros() as u64;
                self.timing.periodic_count += 1;
            }
        }
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    pub fn log_mode(&mut self) {
        let effective_cap = if self.cfg.target_limit > 0 {
            self.cfg.target_limit.min(self.cfg.max_targets)
        } else {
            self.cfg.max_targets
        };
        self.log(
            "MODE",
            format!(
                "MODE=ALL_RADIUS RADIUS={:.1} TARGET_LIMIT={} (0=no_extra_limit) \
                 MAX_TARGETS={} EFFECTIVE_CAP={} HARD_MAX_TARGETS={} \
                 F8_MODE=START_ONLY MAX_LIST_NODES={} (unused_1_17_2) \
                 MAX_POSITION_SCAN={} WCM_VTABLE={:#x} CHR_ARRAYS={}x{} \
                 TEAM_FILTER=NONE PLAYER_EXCLUSION=CHRINS_POINTER \
                 DEBUG_HOTKEY={} DEBUG_LOG={} \
                 HOLD_PROBES={} SCALE={} DURATION_MS={} SPEED_OFFSET={:#x} \
                 WCM_RVA={:#x} POSITION_SOURCE=CSChrPhysicsModule+0x70 \
                 CONFIG_REJECTED={}",
                self.cfg.radius_m, self.cfg.target_limit, self.cfg.max_targets,
                effective_cap, HARD_MAX_TARGETS,
                self.cfg.max_list_nodes, self.cfg.max_candidates,
                WCM_VTABLE_RVA, CHR_ARRAY_FIELDS.len(), CHR_ARRAY_SLOTS,
                self.cfg.hotkey_enabled as u8, self.cfg.debug_log as u8,
                self.cfg.hold_probe_targets, self.cfg.scale, self.cfg.duration_ms,
                self.cfg.speed_offset, self.cfg.wcm_rva,
                self.cfg.forced_halt.is_some() as u8
            ),
        );
    }

    pub fn distinct_write_addresses(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.writes.iter().map(|w| w.addr).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

// ---------------------------------------------------------------------------
// Offline tests. No game process, no real memory.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Offline tests. No game process, no real memory.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const BASE: usize = 0x100_0000;
    const WCM: usize = 0x200_0000;
    const PLAYER: usize = 0x400_0000;
    // 1.17.2 enumeration: three 1500-slot arrays of 16-byte entries.
    // 3 * 1500 * 16 = 0x11940, so 0x20000 apart cannot overlap.
    const ARR0: usize = 0x300_0000;
    const ARR1: usize = 0x302_0000;
    const ARR2: usize = 0x304_0000;
    const ENTRY: usize = CHR_ENTRY_SIZE;
    const CHR_STRIDE: usize = 0x10000;
    const IMG_VT: u64 = (BASE + 0x00A0_0000) as u64;

    struct Mock {
        mem: HashMap<usize, u8>,
        now: u64,
        key: bool,
        writable: bool,
        fail_write: bool,
        write_calls: usize,
        fail_on_call: Option<usize>,
        fail_from: Option<usize>,
        write_cost_ms: u64,
        silent_write: bool,
        read_calls: usize,
        target: usize,
        base: usize,
        /// mirrors WinBackend's ceiling. Counts refused oversized reads.
        read_rejections: usize,
        /// addresses whose read returns None (chunk-failure injection).
        fail_read_addrs: Vec<usize>,
        /// (wcm+field address, value returned on the SECOND read), for
        /// base-change injection. The first read still returns the real value.
        base_override: Option<(usize, usize)>,
        base_reads: usize,
    }

    impl Mock {
        fn put(&mut self, a: usize, v: &[u8]) {
            for (i, b) in v.iter().enumerate() {
                self.mem.insert(a + i, *b);
            }
        }
        fn u8v(&mut self, a: usize, v: u8) { self.mem.insert(a, v); }
        fn i32v(&mut self, a: usize, v: i32) { self.put(a, &v.to_le_bytes()); }
        fn u32v(&mut self, a: usize, v: u32) { self.put(a, &v.to_le_bytes()); }
        fn u64v(&mut self, a: usize, v: u64) { self.put(a, &v.to_le_bytes()); }
        fn f32v(&mut self, a: usize, v: f32) { self.put(a, &v.to_le_bytes()); }
        fn press(&mut self) { self.key = true; }
        fn release(&mut self) { self.key = false; }
        fn advance(&mut self, ms: u64) { self.now += ms; }
    }

    impl Backend for Mock {
        fn now_ms(&self) -> u64 { self.now }
        fn key_down(&self) -> bool { self.key }
        fn target_ptr(&mut self) -> usize { self.target }
        fn module_base(&mut self) -> usize { self.base }
        fn read_bytes(&mut self, a: usize, len: usize) -> Option<Vec<u8>> {
            self.read_calls += 1;
            // the REAL WinBackend refuses these before any memory API.
            // The old Mock happily returned 24000 bytes, which is why 69 tests
            // passed while the deployed DLL could not read a single array.
            if a == 0 || len == 0 || len > MAX_READ_BLOCK {
                self.read_rejections += 1;
                return None;
            }
            if self.fail_read_addrs.contains(&a) {
                return None;
            }
            if let Some((fa, newv)) = self.base_override {
                if a == fa && len == 8 {
                    if self.base_reads > 0 {
                        return Some(newv.to_le_bytes().to_vec());
                    }
                    self.base_reads += 1;
                }
            }
            // Unwritten bytes inside an allocated region read as 0, like real
            // committed-but-uninitialised memory.
            Some((0..len).map(|i| self.mem.get(&(a + i)).copied().unwrap_or(0)).collect())
        }
        fn write_f32(&mut self, a: usize, v: f32) -> Result<(), ()> {
            self.write_calls += 1;
            self.now += self.write_cost_ms;
            if self.fail_write || !self.writable { return Err(()); }
            if self.fail_on_call == Some(self.write_calls) { return Err(()); }
            if let Some(n) = self.fail_from { if self.write_calls >= n { return Err(()); } }
            if self.silent_write {
                // Reports success but the value never lands: exactly what a
                // direct store to a protected page would look like if the OS
                // silently ignored it.
                return Ok(());
            }
            self.f32v(a, v);
            Ok(())
        }
        fn writable(&self, _a: usize) -> bool { self.writable }
    }

    struct Fixture {
        m: Mock,
        enemies: Vec<usize>,
        speed_addrs: Vec<usize>,
    }

    /// Build one ChrIns with a valid physics module at `pos`.
    fn put_chr(m: &mut Mock, p: usize, npc_param: i32, npc_id: i32, chr_type: i32,
               team: u8, container: usize, behavior: usize, physics: usize,
               pos: [f32; 3]) {
        m.u64v(p + OFF_VTABLE, IMG_VT + (p & 0xFFFF) as u64);
        m.i32v(p + OFF_NPC_PARAM_ID, npc_param);
        m.i32v(p + OFF_NPC_ID, npc_id);
        m.i32v(p + OFF_CHR_TYPE, chr_type);
        m.u8v(p + OFF_TEAM_TYPE, team);
        m.u32v(p + OFF_CHARACTER_ID, npc_id as u32);
        m.u64v(p + OFF_MODULES, container as u64);
        // container: slot 0 data module, slot 5 behavior, slot 13 physics
        m.u64v(container, (container + 0x800) as u64);
        m.u64v(container + SLOT_BEHAVIOR_OFF, behavior as u64);
        m.u64v(container + SLOT_PHYSICS_OFF, physics as u64);
        // physics module: vtable in image, owner backref, data module backref
        m.u64v(physics + PHYS_OFF_VTABLE, (BASE + 0x00B0_0000) as u64);
        m.u64v(physics + PHYS_OFF_OWNER, p as u64);
        m.u64v(physics + PHYS_OFF_DATA_MODULE, (container + 0x800) as u64);
        m.f32v(physics + PHYS_OFF_POSITION, pos[0]);
        m.f32v(physics + PHYS_OFF_POSITION + 4, pos[1]);
        m.f32v(physics + PHYS_OFF_POSITION + 8, pos[2]);
        // behavior module
        m.u64v(behavior + BEHAVIOR_OFF_OWNER, p as u64);
        m.f32v(behavior + DEFAULT_SPEED_OFFSET, 1.0);
    }

    /// Enemies stand at (distance, 0, 0) from the player at the origin.
    fn fixture(distances: &[f32]) -> Fixture {
        let mut m = Mock {
            mem: HashMap::new(),
            now: 0, key: false, writable: true, fail_write: false,
            write_calls: 0, fail_on_call: None, fail_from: None, write_cost_ms: 0,
            silent_write: false, read_calls: 0, target: 0, base: BASE,
            read_rejections: 0, fail_read_addrs: Vec::new(),
            base_override: None, base_reads: 0,
        };
        m.u64v(BASE + DEFAULT_WCM_RVA, WCM as u64);
        // The version gate: WorldChrMan's vtable must be the build's own.
        m.u64v(WCM, (BASE + WCM_VTABLE_RVA) as u64);
        m.u64v(WCM + DEFAULT_PLAYER_OFF, PLAYER as u64);
        m.u64v(WCM + CHR_ARRAY_FIELDS[0], ARR0 as u64);
        m.u64v(WCM + CHR_ARRAY_FIELDS[1], ARR1 as u64);
        m.u64v(WCM + CHR_ARRAY_FIELDS[2], ARR2 as u64);

        put_chr(&mut m, PLAYER, 0, 0, 1, PLAYER_TEAM,
                0x50_0000, 0x60_0000, 0x80_0000, [0.0, 0.0, 0.0]);
        m.u64v(ARR0, PLAYER as u64);              // slot 0 of array 0

        let mut en = Vec::new();
        let mut sa = Vec::new();
        for (i, d) in distances.iter().enumerate() {
            let p = PLAYER + (i + 1) * CHR_STRIDE;
            let container = 0x50_1000 + i * 0x1000;
            let behavior = 0x60_1000 + i * 0x1000;
            let physics = 0x80_1000 + i * 0x1000;
            put_chr(&mut m, p, 43_114_010 + i as i32, 4311 + i as i32,
                    CHR_TYPE_NPC, 48, container, behavior, physics,
                    [*d, 0.0, 0.0]);
            en.push(p);
            sa.push(behavior + DEFAULT_SPEED_OFFSET);
            m.u64v(ARR0 + (i + 1) * ENTRY, p as u64);
        }

        // The same character also sits in array 1: dedup has to drop it,
        // otherwise it would be verified (and possibly written) twice.
        if let Some(&first) = en.first() {
            m.u64v(ARR1, first as u64);
        }

        // A non-NPC that lives ONLY in array 2. Proves the union is real (a
        // single-array implementation would never see it) and that the chr_type
        // screen still rejects it.
        let i = distances.len();
        let p_bad = PLAYER + (i + 1) * CHR_STRIDE;
        put_chr(&mut m, p_bad, 9001, 9001, 9, 48,
                0x50_9000, 0x60_9000, 0x80_9000, [1.0, 0.0, 0.0]);
        m.u64v(ARR2, p_bad as u64);

        Fixture { m, enemies: en, speed_addrs: sa }
    }

    /// Append one more ChrIns into a chosen character array. Returns its
    /// ChrIns pointer and speed address. parameterised by array so the
    /// three-array union can be exercised end to end.
    fn append_chr_to(f: &mut Fixture, array: usize, npc_param: i32, npc_id: i32,
                     team: u8, dist: f32) -> (usize, usize) {
        let n = f.enemies.len() + 1;
        let p = PLAYER + (n + 8) * CHR_STRIDE;
        let container = 0x50_C000 + n * 0x1000;
        let behavior = 0x60_C000 + n * 0x1000;
        let physics = 0x80_C000 + n * 0x1000;
        put_chr(&mut f.m, p, npc_param, npc_id, CHR_TYPE_NPC, team,
                container, behavior, physics, [dist, 0.0, 0.0]);
        let mut slot = 0usize;
        loop {
            assert!(slot < CHR_ARRAY_SLOTS, "fixture array is full");
            let mut b = [0u8; 8];
            for k in 0..8 {
                b[k] = f.m.mem.get(&(array + slot * ENTRY + k)).copied().unwrap_or(0);
            }
            if usize::from_le_bytes(b) == 0 {
                break;
            }
            slot += 1;
        }
        f.m.u64v(array + slot * ENTRY, p as u64);
        f.enemies.push(p);
        f.speed_addrs.push(behavior + DEFAULT_SPEED_OFFSET);
        (p, behavior + DEFAULT_SPEED_OFFSET)
    }

    fn append_chr(f: &mut Fixture, npc_param: i32, npc_id: i32, team: u8,
                  dist: f32) -> (usize, usize) {
        append_chr_to(f, ARR0, npc_param, npc_id, team, dist)
    }

    fn cfg(armed: bool) -> Config {
        Config {
            armed,
            player_chr_ptr: 0,
            speed_offset: DEFAULT_SPEED_OFFSET,
            scale: DEFAULT_SCALE,
            duration_ms: DEFAULT_DURATION_MS,
            max_targets: DEFAULT_MAX_TARGETS,
            max_list_nodes: DEFAULT_MAX_LIST_NODES,
            wcm_rva: DEFAULT_WCM_RVA,
            max_candidates: MAX_CANDIDATE_SCAN,
            player_off: DEFAULT_PLAYER_OFF,
            target_limit: 0,
            radius_m: DEFAULT_RADIUS_M,
            page_check: false,
            hold_probe_targets: 0,
            hotkey_enabled: true,
            debug_log: true,
            forced_halt: None,
        }
    }

    #[test]
    fn f1_disarmed_never_writes() {
        let f = fixture(&[5.0]);
        let mut c = Controller::new(cfg(false), f.m);
        c.backend.press(); c.tick(); c.backend.release(); c.tick();
        assert_eq!(c.state, State::Halted("NOT_ARMED"));
        assert!(c.writes.is_empty());
    }

    #[test]
    fn f2_radius_selects_only_near_targets() {
        // 5 m and 20 m inside a 30 m radius; 45 m outside and must stay at 1.0.
        let f = fixture(&[5.0, 20.0, 45.0]);
        let in_near = f.enemies[0];
        let in_far = f.enemies[2];
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        assert_eq!(c.targets.len(), 2, "only the two inside 30 m");
        assert!(c.targets.iter().all(|t| t.chr_ptr != in_far));
        assert_eq!(c.backend.read_f32(speeds[0]), Some(0.2));
        assert_eq!(c.backend.read_f32(speeds[1]), Some(0.2));
        assert_eq!(c.backend.read_f32(speeds[2]), Some(1.0), "45 m untouched");
        let rep = c.last_enum.clone().unwrap();
        assert_eq!(rep.inside_radius, 2);
        assert_eq!(rep.outside_radius, 1);
        assert_eq!(rep.skipped_player, 1);
        assert!(c.targets.iter().any(|t| t.chr_ptr == in_near));
    }

    #[test]
    fn f3_all_targets_restore_after_15s() {
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick(); c.backend.release(); c.tick();
        for a in &speeds { assert_eq!(c.backend.read_f32(*a), Some(0.2)); }
        c.backend.advance(DEFAULT_DURATION_MS + 10); c.tick();
        assert_eq!(c.state, State::Idle);
        for a in &speeds { assert_eq!(c.backend.read_f32(*a), Some(1.0)); }
    }

    #[test]
    fn f4_f8_is_start_only() {
        // A second press while Active must NOT cancel and must not re-scan.
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        let writes_after_activation = c.writes.len();
        c.backend.release(); c.tick();
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active, "second F8 ignored");
        assert_eq!(c.writes.len(), writes_after_activation, "no extra writes");
        for a in &speeds { assert_eq!(c.backend.read_f32(*a), Some(0.2)); }
        c.backend.release(); c.tick();
        c.backend.advance(DEFAULT_DURATION_MS + 10); c.tick();
        assert_eq!(c.state, State::Idle);
        for a in &speeds { assert_eq!(c.backend.read_f32(*a), Some(1.0)); }
    }

    #[test]
    fn f5_midway_write_failure_rolls_back_already_written() {
        let mut f = fixture(&[5.0, 10.0, 15.0]);
        f.m.fail_on_call = Some(3);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Halted("ACTIVATION_WRITE_FAILED_ROLLED_BACK"));
        for a in &speeds { assert_eq!(c.backend.read_f32(*a), Some(1.0)); }
        assert!(c.events.iter().any(|e| e.kind == "ROLLBACK"));
    }

    #[test]
    fn f6_partial_identity_loss_forbids_only_that_write() {
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let second = f.enemies[1];
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        c.backend.release(); c.tick();
        // The second target's module container now points somewhere else: the
        // object can no longer be attributed, so its address must not be written.
        c.backend.u64v(second + OFF_MODULES, 0x1234_5678);
        c.backend.advance(DEFAULT_DURATION_MS + 10); c.tick();
        // unknown identity is a per-target outcome. The healthy target is
        // restored, the unknown one is left alone, and the controller returns to
        // Idle so the next trigger still works (no more whole-session halt).
        assert_eq!(c.state, State::Idle);
        assert_eq!(c.backend.read_f32(speeds[0]), Some(1.0), "healthy one restored");
        assert_eq!(c.backend.read_f32(speeds[1]), Some(0.2), "unknown one untouched");
        assert!(c.events.iter().any(|e| e.kind == "FORBIDDEN_TARGETS_LEFT_ALONE"));
    }

    #[test]
    fn f6b_rebuilt_container_is_reresolved_and_restored() {
        // A boss whose physics/behavior module is rebuilt mid-window: the cached
        // speed address is stale, but the character is still the same. The undo
        // must follow the chain from chr_ptr instead of leaving it slowed.
        let f = fixture(&[5.0]);
        let chr = f.enemies[0];
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        c.backend.release(); c.tick();
        assert_eq!(c.backend.read_f32(f.speed_addrs[0]), Some(0.2), "slowed");

        let new_container = 0x50_D000usize;
        let new_behavior = 0x60_D000usize;
        let new_addr = new_behavior + DEFAULT_SPEED_OFFSET;
        c.backend.u64v(chr + OFF_MODULES, new_container as u64);
        c.backend.u64v(new_container + SLOT_BEHAVIOR_OFF, new_behavior as u64);
        c.backend.u64v(new_behavior + BEHAVIOR_OFF_OWNER, chr as u64);
        // The rebuilt module inherited the slow value we wrote earlier.
        c.backend.f32v(new_addr, 0.2);

        c.backend.advance(DEFAULT_DURATION_MS + 10); c.tick();
        assert_eq!(c.state, State::Idle);
        assert_eq!(c.backend.read_f32(new_addr), Some(1.0),
                   "undo followed the rebuilt chain");
        assert!(c.events.iter().any(|e| e.kind == "TARGET_RESTORED_RERESOLVED"));
        assert_eq!(c.events.iter().filter(|e| e.kind == "FORBIDDEN_TARGETS_LEFT_ALONE").count(), 0);
    }

    #[test]
    fn f6c_reresolve_refuses_foreign_field_owner() {
        // Same shape, but the rebuilt behavior belongs to somebody else: the
        // re-derived field must NOT be written.
        let f = fixture(&[5.0]);
        let chr = f.enemies[0];
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        c.backend.release(); c.tick();

        let new_container = 0x50_E000usize;
        let new_behavior = 0x60_E000usize;
        let new_addr = new_behavior + DEFAULT_SPEED_OFFSET;
        c.backend.u64v(chr + OFF_MODULES, new_container as u64);
        c.backend.u64v(new_container + SLOT_BEHAVIOR_OFF, new_behavior as u64);
        // owner back-reference points at a different chr
        c.backend.u64v(new_behavior + BEHAVIOR_OFF_OWNER, 0xDEAD_0000u64);
        c.backend.f32v(new_addr, 0.2);

        c.backend.advance(DEFAULT_DURATION_MS + 10); c.tick();
        assert_eq!(c.state, State::Idle);
        assert_eq!(c.backend.read_f32(new_addr), Some(0.2), "foreign field untouched");
        assert!(c.events.iter().any(|e| e.kind == "FORBIDDEN_TARGETS_LEFT_ALONE"));
    }

    #[test]
    fn f6d_manual_sweep_undoes_leftover_slow_values() {
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        c.backend.release(); c.tick();
        assert_eq!(c.backend.read_f32(speeds[0]), Some(0.2));
        // Simulate the worst case: the target list was lost without a restore.
        c.targets.clear();
        c.displaced.clear();
        c.state = State::Idle;
        assert_eq!(c.force_restore_sweep(), 2, "both leftovers undone");
        for a in &speeds {
            assert_eq!(c.backend.read_f32(*a), Some(1.0));
        }
        assert_eq!(c.force_restore_sweep(), 0, "nothing left to undo");
        assert!(c.events.iter().any(|e| e.kind == "FORCE_RESTORE_SWEEP"));
    }

    #[test]
    fn f6e_rescan_adopts_characters_that_appear_later() {
        // A boss spawns after activation (fog gate). Without the mid-window
        // rescan it would never be slowed, because activation is a snapshot.
        let f = fixture(&[5.0]);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);

        let p = PLAYER + 40 * CHR_STRIDE;
        let container = 0x50_F000usize;
        let behavior = 0x60_F000usize;
        let physics = 0x80_F000usize;
        put_chr(&mut c.backend, p, 1_000_000, 1000, CHR_TYPE_NPC, 48,
                container, behavior, physics, [12.0, 0.0, 0.0]);
        c.backend.u64v(ARR0 + 40 * ENTRY, p as u64);

        c.backend.advance(RESCAN_MS + 20);
        c.tick();
        assert!(c.events.iter().any(|e| e.kind == "TARGET_ADDED_LATE"),
                "events: {:?}", c.events);
        assert_eq!(c.backend.read_f32(behavior + DEFAULT_SPEED_OFFSET), Some(0.2),
                   "late arrival is slowed");
        assert!(c.targets.iter().any(|t| t.chr_ptr == p));

        c.backend.advance(DEFAULT_DURATION_MS);
        c.tick();
        assert_eq!(c.state, State::Idle);
        assert_eq!(c.backend.read_f32(behavior + DEFAULT_SPEED_OFFSET), Some(1.0),
                   "late arrival is restored too");
    }

    #[test]
    fn f6f_boss_chr_type_is_targeted() {
        // Margit reads chr_type 7. The old `== 5` screen threw him away; the
        // activation must now slow him like any NPC.
        let mut f = fixture(&[5.0]);
        let p = f.enemies[0];
        f.m.i32v(p + OFF_CHR_TYPE, CHR_TYPE_BOSS);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press();
        c.tick();
        assert_eq!(c.state, State::Active);
        assert_eq!(c.backend.read_f32(speeds[0]), Some(0.2), "boss is slowed");
        assert!(c.events.iter().any(|e| e.kind == "BOSS_TYPE_TARGET"),
                "events: {:?}", c.events);
        c.backend.release();
        c.backend.advance(DEFAULT_DURATION_MS + 10);
        c.tick();
        assert_eq!(c.backend.read_f32(speeds[0]), Some(1.0), "boss restored");
    }

    #[test]
    fn f7_no_target_in_radius_is_retryable() {        // Every enemy is far outside the radius: nothing to do, stay Idle.
        let f = fixture(&[100.0, 120.0]);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Idle);
        assert!(c.writes.is_empty());
    }

    #[test]
    fn f8_null_wcm_fails_closed() {
        let mut f = fixture(&[5.0]);
        f.m.u64v(BASE + DEFAULT_WCM_RVA, 0);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Halted("WCM_NULL"));
        assert!(c.writes.is_empty());
    }

    #[test]
    fn f9_shutdown_restores_all() {
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert!(c.restore_now());
        for a in &speeds { assert_eq!(c.backend.read_f32(*a), Some(1.0)); }
    }

    #[test]
    fn f10_sentinel_head_is_skipped_and_walk_continues() {
        let mut f = fixture(&[5.0, 10.0]);
        // Slot 0 becomes junk (its "vtable" is outside the image). The screen
        // must skip it and carry on with the remaining slots.
        f.m.u64v(ARR0, 0xDEAD_BEEF_u64);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        assert_eq!(c.targets.len(), 2, "walk continued past the sentinel");
        assert!(c.last_enum.clone().unwrap().skipped_invalid >= 1);
    }

    #[test]
    fn f11_control_failed_is_not_written_back() {
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        c.backend.release(); c.tick();
        c.backend.f32v(speeds[0], f32::NAN);
        c.backend.advance(DEFAULT_DURATION_MS + 10); c.tick();
        assert_eq!(c.state, State::Halted("RESTORE_FAILED__RESTART_GAME"));
        assert!(c.requires_game_restart());
        assert!(c.backend.read_f32(speeds[0]).unwrap().is_nan());
        assert_eq!(c.backend.read_f32(speeds[1]), Some(1.0));
    }

    #[test]
    fn f12_rollback_failure_is_reported_not_hidden() {
        let mut f = fixture(&[5.0, 10.0, 15.0]);
        f.m.fail_from = Some(3);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(
            c.state,
            State::Halted("ACTIVATION_WRITE_FAILED_ROLLBACK_INCOMPLETE__RESTART_GAME")
        );
        assert!(c.requires_game_restart());
        assert!(c.events.iter().any(|e| e.kind == "ROLLBACK_INCOMPLETE"));
    }

    #[test]
    fn f13_timer_starts_after_writes() {
        let mut f = fixture(&[5.0, 10.0]);
        f.m.write_cost_ms = 200; // 2 targets -> 400 ms of writing
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        // 400 ms already spent; 14.9 s more must NOT expire the effect.
        c.backend.release(); c.tick();
        c.backend.advance(14_900); c.tick();
        assert_eq!(c.state, State::Active, "timer must start after the writes");
        c.backend.advance(200); c.tick();
        assert_eq!(c.state, State::Idle);
    }

    #[test]
    fn f14_nearest_first_then_limit() {
        // Chain order is far -> near; the limit must keep the NEAREST ones.
        let f = fixture(&[25.0, 5.0, 15.0]);
        let mut conf = cfg(true);
        conf.target_limit = 2;
        let mut c = Controller::new(conf, f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.targets.len(), 2);
        let d: Vec<f32> = c.targets.iter().map(|t| t.distance_m).collect();
        assert!((d[0] - 5.0).abs() < 0.01, "nearest first, got {:?}", d);
        assert!((d[1] - 15.0).abs() < 0.01, "second nearest, got {:?}", d);
        assert!(c.targets.iter().all(|t| t.distance_m < 20.0),
                "the 25 m one must be dropped by the limit");
    }

    #[test]
    fn f15_hold_probes_record_field_values() {
        let f = fixture(&[5.0]);
        let mut conf = cfg(true);
        conf.hold_probe_targets = 1;
        let mut c = Controller::new(conf, f.m);
        c.backend.press(); c.tick();
        assert!(c.holds.iter().any(|h| h.tag == "immediate"
                && (h.value.unwrap_or(0.0) - 0.2).abs() < 1e-6));
        c.backend.release(); c.tick();
        c.backend.advance(600); c.tick();
        assert!(c.holds.iter().any(|h| h.tag == "t500"));
    }

    #[test]
    fn f16_player_position_failure_refuses_activation() {
        let mut f = fixture(&[5.0, 10.0]);
        // Break the player's physics slot: position cannot be established.
        f.m.u64v(0x50_0000 + SLOT_PHYSICS_OFF, 0);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Idle, "refused, not degraded to map-wide");
        assert!(c.writes.is_empty());
        assert!(c.events.iter().any(|e| e.kind == "ACTIVATION_REFUSED"
                && e.detail.starts_with("PLAYER_")));
    }

    #[test]
    fn f17_invalid_radius_halts() {
        let f = fixture(&[5.0]);
        let mut conf = cfg(true);
        conf.radius_m = 0.0;
        let mut c = Controller::new(conf, f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Halted("INVALID_RADIUS"));
        assert!(c.writes.is_empty());
    }

    #[test]
    fn f18_boundary_is_inclusive() {
        // Exactly on the radius: d2 == r2 must be accepted.
        let f = fixture(&[30.0]);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.targets.len(), 1);
        assert!((c.targets[0].distance_m - 30.0).abs() < 0.001);
    }

    #[test]
    fn f19_limit_stops_verification_early() {
        // With 3 candidates inside the radius, asking for 1 target must NOT
        // pay for the other two BehaviorModule verifications.
        let f1 = fixture(&[5.0, 10.0, 15.0]);
        let mut c1 = cfg(true);
        c1.target_limit = 1;
        let mut one = Controller::new(c1, f1.m);
        one.backend.press(); one.tick();
        let reads_one = one.backend.read_calls;
        assert_eq!(one.targets.len(), 1);

        let f3 = fixture(&[5.0, 10.0, 15.0]);
        let mut c3 = cfg(true);
        c3.target_limit = 3;
        let mut three = Controller::new(c3, f3.m);
        three.backend.press(); three.tick();
        let reads_three = three.backend.read_calls;
        assert_eq!(three.targets.len(), 3);

        assert!(reads_one < reads_three,
                "limit=1 must read less than limit=3 ({} vs {})",
                reads_one, reads_three);
    }

    #[test]
    fn f20_silent_write_is_caught_by_readback() {
        // The direct store path has no API return value, so the read-back is
        // the only thing standing between us and a silent no-op. Prove it fires.
        let mut f = fixture(&[5.0, 10.0]);
        f.m.silent_write = true;
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Halted("ACTIVATION_WRITE_FAILED_ROLLED_BACK"));
        assert!(c.writes.is_empty(), "no write may be recorded as done");
        for a in &speeds {
            assert_eq!(c.backend.read_f32(*a), Some(1.0), "value untouched");
        }
    }

    // ---- ------------------------------------------------------------

    #[test]
    fn f21_same_team_npc_is_included() {
        // Team filtering is gone: an NPC standing on the player's own team 2 m
        // away is a perfectly valid target and MUST be slowed.
        let mut f = fixture(&[5.0]);
        let (_p, addr) = append_chr(&mut f, 9002, 9002, PLAYER_TEAM, 2.0);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        assert_eq!(c.targets.len(), 2, "same-team NPC included, got {:?}",
                   c.targets.iter().map(|t| t.distance_m).collect::<Vec<_>>());
        assert_eq!(c.backend.read_f32(addr), Some(0.2));
        assert!(c.last_enum.as_ref().unwrap().skipped_player == 1,
                "only the real player is pointer-excluded");
    }

    #[test]
    fn f22_cap_drop_is_recorded_not_silent() {
        // 6 in-radius characters, budget 3: three must be written and the other
        // three must appear in dropped_cap. Nothing may vanish without a count.
        let f = fixture(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let mut conf = cfg(true);
        conf.target_limit = 0;      // "no extra limit"
        conf.max_targets = 3;       // ...but the hard cap still applies
        let mut c = Controller::new(conf, f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        let rep = c.last_enum.clone().unwrap();
        assert_eq!(rep.inside_radius, 6);
        assert_eq!(rep.cap, 3);
        assert_eq!(rep.written, 3);
        assert_eq!(rep.dropped_cap, 3, "3 in-radius targets left untouched");
        assert_eq!(c.targets.len(), 3);
        assert!(c.events.iter().any(|e| e.kind == "CAP_DROPPED"));
    }

    #[test]
    fn f23_scan_truncation_is_recorded() {
        // More distinct candidates than the configured ceiling: the overflow is
        // counted in dropped_scan and announced, never silently discarded.
        let mut f = fixture(&[]);
        for _ in 0..2 {
            append_chr(&mut f, 4311, 4311, 48, 100.0);
        }
        // fixture(&[]) has 1 candidate (the array-2 non-NPC); +2 appended = 3
        let mut conf = cfg(true);
        conf.max_candidates = 2;
        conf.radius_m = 200.0;
        let mut c = Controller::new(conf, f.m);
        c.backend.press(); c.tick();
        let rep = c.last_enum.clone().unwrap();
        assert_eq!(rep.candidates, 3);
        assert_eq!(rep.dropped_scan, 1, "exactly one over the ceiling");
        assert!(c.events.iter().any(|e| e.kind == "SCAN_TRUNCATED"));
    }

    #[test]
    fn f24_arrays_are_unioned_and_deduplicated() {
        // The same character sits in array 0 and array 1; a character that only
        // exists in array 2 must still be found. This is the whole point of
        // scanning three arrays instead of one.
        let f = fixture(&[1.0, 2.0, 3.0]);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        let rep = c.last_enum.clone().unwrap();
        assert_eq!(rep.arrays_read, 3, "all three arrays present");
        assert_eq!(rep.duplicates, 1, "the duplicated ChrIns was dropped once");
        assert_eq!(rep.candidates, 4,
                   "3 enemies + the array-2 non-NPC, duplicate removed");
        assert_eq!(rep.slots_scanned, CHR_ARRAY_SLOTS * 3);
        assert_eq!(c.targets.len(), 3, "only the three enemies survive screening");
    }

    #[test]
    fn f35_wrong_world_chr_man_vtable_halts_with_a_named_reason() {
        // The one thing worse than failing is failing silently. A different exe
        // must produce GAME_VERSION_MISMATCH, not a confident read of garbage.
        let mut f = fixture(&[5.0]);
        f.m.u64v(WCM, 0xDEAD_0000_u64);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Halted("GAME_VERSION_MISMATCH"));
        assert!(c.writes.is_empty(), "nothing may be written");
        assert!(c.events.iter().any(|e| e.kind == "GAME_VERSION_MISMATCH"));
        let ev = c.events.iter().find(|e| e.kind == "GAME_VERSION_MISMATCH").unwrap();
        assert!(ev.detail.contains("expected_vtable"));
    }

    #[test]
    fn f36_empty_arrays_are_retryable_not_fatal() {
        // Main menu / mid-load: WorldChrMan exists but no array is populated.
        // That must stay retryable; halting would kill the mod for the session.
        let mut f = fixture(&[5.0]);
        for field in CHR_ARRAY_FIELDS {
            f.m.u64v(WCM + field, 0);
        }
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Idle, "must stay armed for the next try");
        assert!(c.events.iter().any(|e| e.kind == "ACTIVATION_REFUSED"
            && e.detail.contains("NO_CHR_ARRAYS")));
    }

    #[test]
    fn f25_npc_param_zero_is_auxiliary_only() {
        // Identity rests on vtable + owner backrefs + character_id cross-check.
        // npc_param_id == 0 is counted, never used to veto the write.
        let mut f = fixture(&[5.0]);
        let p = f.enemies[0];
        f.m.i32v(p + OFF_NPC_PARAM_ID, 0);
        let addr = f.speed_addrs[0];
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        assert_eq!(c.backend.read_f32(addr), Some(0.2));
        assert_eq!(c.last_enum.as_ref().unwrap().aux_npc_param_zero, 1);
        assert!(c.events.iter().any(|e| e.kind == "NPC_PARAM_ID_ZERO_AUX"));
    }

    #[test]
    fn f26_unresolvable_player_pointer_refuses_activation() {
        // No player pointer anywhere: refuse rather than slow everything.
        let mut f = fixture(&[5.0, 10.0]);
        f.m.u64v(WCM + DEFAULT_PLAYER_OFF, 0);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Idle, "refused, retryable");
        assert!(c.writes.is_empty());
        assert!(c.events.iter().any(|e| e.kind == "ACTIVATION_REFUSED"
                && e.detail == "PLAYER_UNKNOWN"));
    }

    #[test]
    fn f27_all_mode_four_counts_chain() {
        // inside_radius >= verified >= written, and
        // inside_radius == written + dropped_cap + verify_failures.
        // Nearest (1 m) has a broken behavior owner -> verify fails;
        // the 4 m one never gets verified because the cap of 2 is reached.
        let mut f = fixture(&[1.0, 2.0, 3.0]);
        let (_p4, addr4) = append_chr(&mut f, 4311, 4311, 48, 4.0);
        let broken_behavior = 0x60_1000; // enemies[0] at 1 m
        f.m.u64v(broken_behavior + BEHAVIOR_OFF_OWNER, 0xDEAD);
        let addr_near = f.speed_addrs[0];
        let mut conf = cfg(true);
        conf.target_limit = 0;
        conf.max_targets = 2;
        let mut c = Controller::new(conf, f.m);
        c.backend.press(); c.tick();
        assert_eq!(c.state, State::Active);
        let rep = c.last_enum.clone().unwrap();
        assert_eq!(rep.inside_radius, 4);
        assert_eq!(rep.verified, rep.written, "no write failed here");
        assert_eq!(rep.written, 2);
        assert_eq!(rep.dropped_cap, 1, "the 4 m one was never verified");
        assert_eq!(rep.inside_radius, rep.written + rep.dropped_cap + 1,
                   "the 1 m candidate failed verification");
        assert_eq!(c.backend.read_f32(addr_near), Some(1.0),
                   "verification failure must never be written");
        assert_eq!(c.backend.read_f32(addr4), Some(1.0), "cap-dropped stays at 1.0");
    }

    // ---- configuration-driven behaviour -----------------------------

    #[test]
    fn f28_disabled_hotkey_never_activates_and_says_so_once() {
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut conf = cfg(true);
        conf.hotkey_enabled = false;
        let mut c = Controller::new(conf, f.m);
        for _ in 0..3 {
            c.backend.press();
            c.tick();
            c.backend.release();
            c.tick();
        }
        assert_eq!(c.state, State::Idle, "hotkey off must never start an activation");
        assert!(c.writes.is_empty());
        for a in &speeds {
            assert_eq!(c.backend.read_f32(*a), Some(1.0), "nothing may be written");
        }
        let n = c.events.iter().filter(|e| e.kind == "HOTKEY_DISABLED").count();
        assert_eq!(n, 1, "logged once, not once per press");
    }

    #[test]
    fn f29_config_error_halts_instead_of_running_with_a_surprise_value() {
        let f = fixture(&[5.0]);
        let mut conf = cfg(true);
        conf.forced_halt = Some("CONFIG_INVALID_RADIUS");
        let mut c = Controller::new(conf, f.m);
        assert_eq!(c.state, State::Halted("CONFIG_INVALID_RADIUS"));
        c.backend.press();
        c.tick();
        assert!(c.writes.is_empty(), "a rejected config must not slow anything");
    }

    #[test]
    fn f30_debug_log_off_drops_probes_but_keeps_the_safety_trail() {
        let f = fixture(&[5.0, 10.0]);
        let mut conf = cfg(true);
        conf.debug_log = false;
        conf.hold_probe_targets = 2;
        let mut c = Controller::new(conf, f.m);
        c.backend.press();
        c.tick();
        assert_eq!(c.state, State::Active);
        assert_eq!(c.targets.len(), 2, "slowing still happens");

        let kinds: Vec<&str> = c.events.iter().map(|e| e.kind).collect();
        assert!(!kinds.contains(&"PERF"), "PERF is diagnostic only");
        assert!(!kinds.contains(&"TIMING"), "TIMING is diagnostic only");
        assert!(kinds.contains(&"TARGET_WRITTEN"), "writes are never hidden");
        assert!(kinds.contains(&"ACTIVATED"), "activation is never hidden");

        c.backend.release();
        c.tick();
        c.backend.advance(DEFAULT_DURATION_MS + 10);
        c.tick();
        assert_eq!(c.state, State::Idle);
        assert!(c.events.iter().any(|e| e.kind == "RESTORE_SUMMARY"));
        assert!(c.events.iter().any(|e| e.kind == "TARGET_RESTORED"));
    }

    #[test]
    fn f31_same_gate_for_every_trigger_source() {
        // Whatever the source, Idle starts, Active rejects, and the source is
        // recorded. This is what stops a pill used mid-window from extending it.
        let f = fixture(&[5.0, 10.0]);
        let mut c = Controller::new(cfg(true), f.m);
        assert!(c.request_activation(TriggerSource::Item, 0));
        assert_eq!(c.state, State::Active);
        let writes_after = c.writes.len();

        assert!(!c.request_activation(TriggerSource::WeaponSkill, 100));
        assert!(!c.request_activation(TriggerSource::DebugHotkey, 200));
        assert_eq!(c.writes.len(), writes_after, "no extra writes from rejections");
        assert_eq!(c.state, State::Active, "still exactly one active window");

        let accepted: Vec<&str> = c
            .events
            .iter()
            .filter(|e| e.kind == "TRIGGER_ACCEPTED")
            .map(|e| e.detail.as_str())
            .collect();
        assert_eq!(accepted, vec!["source=Item"]);

        let rejected: Vec<&str> = c
            .events
            .iter()
            .filter(|e| e.kind == "TRIGGER_REJECTED")
            .map(|e| e.detail.as_str())
            .collect();
        assert_eq!(rejected.len(), 2);
        assert!(rejected[0].contains("source=WeaponSkill"));
        assert!(rejected[0].contains("ALREADY_ACTIVE"));
        assert!(rejected[1].contains("source=DebugHotkey"));
    }

    #[test]
    fn f32_a_halted_controller_cannot_be_revived_by_a_trigger() {
        let f = fixture(&[5.0]);
        let mut conf = cfg(true);
        conf.forced_halt = Some("CONFIG_INVALID_SCALE");
        let mut c = Controller::new(conf, f.m);
        assert!(!c.request_activation(TriggerSource::Item, 0));
        assert_eq!(c.state, State::Halted("CONFIG_INVALID_SCALE"));
        assert!(c.events.iter().any(|e| e.kind == "TRIGGER_REJECTED"
            && e.detail.contains("HALTED:CONFIG_INVALID_SCALE")));
        assert!(c.writes.is_empty());
    }

    // ---- item edge -> core wiring safety ---------------------------

    #[test]
    fn f37_second_item_edge_during_window_is_refused_with_a_named_reason() {
        // "Every rising edge starts at most one activation": Active rejects,
        // and the host can read the exact reason instead of `started=0`.
        let f = fixture(&[5.0, 10.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        assert!(c.request_activation(TriggerSource::Item, 0));
        assert_eq!(c.state, State::Active);
        let writes_after = c.writes.len();

        assert!(!c.request_activation(TriggerSource::Item, 100));
        assert_eq!(c.last_refusal(), Some("ALREADY_ACTIVE"));
        assert_eq!(c.writes.len(), writes_after, "no second write");
        assert_eq!(c.state, State::Active, "window is neither extended nor restarted");
        for a in &speeds {
            assert_eq!(c.backend.read_f32(*a), Some(0.2), "still slowed exactly once");
        }
        assert!(c.events.iter().any(|e| e.kind == "TRIGGER_REJECTED"
            && e.detail.contains("source=Item")
            && e.detail.contains("ALREADY_ACTIVE")));
    }

    #[test]
    fn f38_item_trigger_uses_the_same_15s_window_and_auto_restore() {
        let f = fixture(&[3.0, 7.0, 12.0]);
        let speeds = f.speed_addrs.clone();
        let mut c = Controller::new(cfg(true), f.m);
        assert!(c.request_activation(TriggerSource::Item, 0));
        assert_eq!(c.targets.len(), 3);
        assert!(c.is_active());
        for a in &speeds {
            assert_eq!(c.backend.read_f32(*a), Some(0.2));
        }

        // A pill mid-flight cannot move the deadline.
        assert!(!c.request_activation(TriggerSource::Item, DEFAULT_DURATION_MS - 1));
        c.backend.advance(DEFAULT_DURATION_MS + 10);
        c.tick();
        assert_eq!(c.state, State::Idle);
        for a in &speeds {
            assert_eq!(c.backend.read_f32(*a), Some(1.0), "restored by AUTO_TIMEOUT");
        }
        assert!(c.events.iter().any(|e| e.kind == "RESTORE_SUMMARY"
            && e.detail.contains("restored=3")
            && e.detail.contains("external_skipped=0")));
    }

    #[test]
    fn f39_restore_never_overwrites_a_speed_changed_by_someone_else() {
        // An external system changes one of our speed fields during the window.
        // Restoring `original` on top of it would destroy that state; the field
        // must be left untouched and the event must say so.
        let f = fixture(&[5.0, 10.0]);
        let a0 = f.speed_addrs[0];
        let a1 = f.speed_addrs[1];
        let mut c = Controller::new(cfg(true), f.m);
        assert!(c.request_activation(TriggerSource::Item, 0));
        c.backend.f32v(a1, 0.75);          // external write, NOT via our backend record
        assert_eq!(c.backend.read_f32(a1), Some(0.75));

        c.backend.advance(DEFAULT_DURATION_MS + 10);
        c.tick();

        assert_eq!(c.backend.read_f32(a0), Some(1.0), "untouched field restored");
        assert_eq!(c.backend.read_f32(a1), Some(0.75), "external value preserved");
        assert_eq!(c.state, State::Idle, "not a failure, not a halt");
        assert!(c.events.iter().any(|e| e.kind == "TARGET_NO_RESTORE_EXTERNAL"
            && e.detail.contains("observed=0.75")));
        assert!(c.events.iter().any(|e| e.kind == "RESTORE_SUMMARY"
            && e.detail.contains("external_skipped=1")));
    }

    #[test]
    fn f40_failed_activation_exposes_a_concrete_reason() {
        // Empty character arrays (menu / mid-load) are retryable, but the item
        // trigger must still be able to say WHY nothing started.
        let mut f = fixture(&[5.0]);
        for field in CHR_ARRAY_FIELDS {
            f.m.u64v(WCM + field, 0);
        }
        let mut c = Controller::new(cfg(true), f.m);
        assert!(!c.request_activation(TriggerSource::Item, 0));
        assert_eq!(c.last_refusal(), Some("NO_CHR_ARRAYS"));
        assert_eq!(c.state, State::Idle, "retryable, not halted");
        assert!(c.writes.is_empty());
    }

    #[test]
    fn f41_same_speed_addr_is_only_claimed_once() {
        // The guard is defensive: today's strict `owner == chr_ptr` check makes
        // two distinct ChrIns share a BehaviorModule impossible. Test the guard
        // itself so a future relaxation of that check cannot silently reintroduce the "original becomes the already-slowed value" bug.
        let mut seen = std::collections::HashSet::new();
        assert!(claim_speed_addr(&mut seen, 0x1000));
        assert!(!claim_speed_addr(&mut seen, 0x1000), "second claim refused");
        assert!(claim_speed_addr(&mut seen, 0x2000));
        assert_eq!(seen.len(), 2);
        let rep = EnumReport::default();
        assert_eq!(rep.duplicate_speed_addrs, 0, "field is wired and starts at zero");
    }

    // ---- chunked ChrArray reads + calibrated Mock ------------------

    #[test]
    fn p3_01_single_24000_read_is_refused_by_mock() {
        // The exact thing that made the deployed DLL return NO_CHR_ARRAYS.
        // The Mock now behaves like WinBackend instead of pretending 4096
        // does not exist.
        let f = fixture(&[5.0]);
        let mut m = f.m;
        let total = CHR_ARRAY_SLOTS * CHR_ENTRY_SIZE;
        assert_eq!(total, 24000);
        assert!(m.read_bytes(ARR0, total).is_none(), "oversized read must be refused");
        assert_eq!(m.read_rejections, 1);
        assert!(m.read_bytes(ARR0, MAX_READ_BLOCK).is_some());
        assert_eq!(m.read_rejections, 1, "a legal read is not a rejection");
    }

    #[test]
    fn p3_02_chunked_24000_read_is_exact() {
        let total = CHR_ARRAY_SLOTS * CHR_ENTRY_SIZE;
        let plan = array_chunk_plan(total);
        assert_eq!(plan.len(), 6, "6 chunks, not 5.86 rounded up by hand");
        let mut expected_off = 0usize;
        for (off, n) in &plan {
            assert_eq!(*off, expected_off, "chunks must be contiguous, no overlap or gap");
            assert!(*n > 0 && *n <= MAX_READ_BLOCK, "every chunk <= 4096");
            expected_off += *n;
        }
        assert_eq!(expected_off, total, "no byte left behind");

        let mut f = fixture(&[5.0]);
        let expect: Vec<u8> = (0..total).map(|i| (i % 251) as u8).collect();
        for (i, b) in expect.iter().enumerate() {
            f.m.u8v(ARR0 + i, *b);
        }
        let got = read_array_chunked(&mut f.m, ARR0, total).unwrap();
        assert_eq!(got.len(), 24000);
        assert_eq!(got, expect, "byte-for-byte identical in the original order");
        assert_eq!(f.m.read_rejections, 0, "no chunk exceeded the ceiling");
    }

    #[test]
    fn p3_03_chunk_table_matches_the_task_book() {
        let want = vec![(0, 4096), (4096, 4096), (8192, 4096),
                        (12288, 4096), (16384, 4096), (20480, 3520)];
        assert_eq!(array_chunk_plan(24000), want);
        assert_eq!(array_chunk_plan(24000).last().unwrap().1, 3520);
        assert_eq!(array_chunk_plan(MAX_READ_BLOCK), vec![(0, 4096)]);
        assert_eq!(array_chunk_plan(MAX_READ_BLOCK + 1),
                   vec![(0, 4096), (4096, 1)]);
    }

    #[test]
    fn p3_04_middle_chunk_failure_discards_the_whole_array() {
        // Function level: the third chunk names itself precisely.
        let mut f = fixture(&[5.0]);
        f.m.fail_read_addrs.push(ARR0 + 8192);
        let e = read_array_chunked(&mut f.m, ARR0, 24000).unwrap_err();
        assert_eq!(e.chunk_index, 2);
        assert_eq!(e.offset, 8192);
        assert_eq!(e.len, 4096);
        assert_eq!(e.reason, "READ_REJECTED");

        // End to end: all three arrays lose their third chunk. The two
        // successful chunks of each array must NOT be parsed, and arrays_read
        // must stay at zero.
        let mut f2 = fixture(&[5.0, 10.0]);
        for a in [ARR0, ARR1, ARR2] {
            f2.m.fail_read_addrs.push(a + 8192);
        }
        let mut c = Controller::new(cfg(true), f2.m);
        c.backend.press();
        c.tick();
        assert_eq!(c.state, State::Idle, "NO_CHR_ARRAYS stays retryable");
        assert!(c.writes.is_empty(), "no partial array may ever reach a write");
        let unre: Vec<&str> = c.events.iter()
            .filter(|e| e.kind == "CHR_ARRAY_UNREADABLE")
            .map(|e| e.detail.as_str()).collect();
        assert_eq!(unre.len(), 3);
        assert!(unre.iter().all(|d| d.contains("chunk=2")
            && d.contains("offset=8192") && d.contains("len=4096")
            && d.contains("whole_array_discarded")), "detail: {:?}", unre);
        assert!(c.events.iter().any(|e| e.kind == "ACTIVATION_REFUSED"
            && e.detail == "NO_CHR_ARRAYS"));
        assert!(!c.events.iter().any(|e| e.kind == "ACTIVATED"));
    }

    #[test]
    fn p3_05_array_base_change_is_retried_not_discarded() {
        // Unchanged base: all three arrays are used.
        let f = fixture(&[5.0, 10.0]);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press();
        c.tick();
        assert_eq!(c.state, State::Active);
        let ok = c.last_enum.clone().unwrap();
        assert_eq!(ok.arrays_read, 3);
        assert_eq!(ok.arrays_base_changed, 0);

        // Array 0's WCM pointer moves during the read (area streaming).
        // retries instead of discarding 1500 characters; if the retry is not
        // done, the local enemies silently disappear from the target list.
        let mut f2 = fixture(&[5.0, 10.0]);
        let enemies = f2.enemies.clone();
        let moved = 0xDEAD_0000usize;
        f2.m.u64v(moved, PLAYER as u64);
        for (i, en) in enemies.iter().enumerate() {
            f2.m.u64v(moved + (i + 1) * ENTRY, *en as u64);
        }
        f2.m.base_override = Some((WCM + CHR_ARRAY_FIELDS[0], moved));
        let mut c2 = Controller::new(cfg(true), f2.m);
        c2.backend.press();
        c2.tick();
        let rep = c2.last_enum.clone().unwrap();
        assert_eq!(rep.arrays_read, 3, "retry recovers the moved array");
        assert!(
            c2.events.iter().any(|e| e.kind == "CHR_ARRAY_BASE_CHANGED"
                && e.detail.contains("field=0x1e6a0")
                && e.detail.contains("note=retrying")),
            "events: {:?}", c2.events
        );
        assert!(rep.verified >= 2, "enemies from the moved array are still targeted");
    }

    #[test]
    fn p3_06_three_arrays_read_merged_and_deduplicated() {
        let mut f = fixture(&[1.0, 2.0, 3.0]);
        // ARR0: player + 3 enemies. ARR1: a duplicate of enemy0 + 2 new.
        // ARR2: one non-NPC + 1 new.
        append_chr_to(&mut f, ARR1, 43_114_020, 4321, 48, 4.0);
        append_chr_to(&mut f, ARR1, 43_114_021, 4322, 48, 5.0);
        append_chr_to(&mut f, ARR2, 43_114_022, 4323, 48, 6.0);
        let mut c = Controller::new(cfg(true), f.m);
        c.backend.press();
        c.tick();
        assert_eq!(c.state, State::Active);
        let rep = c.last_enum.clone().unwrap();
        assert_eq!(rep.arrays_read, 3);
        assert_eq!(rep.slots_scanned, CHR_ARRAY_SLOTS * 3);
        assert_eq!(rep.duplicates, 1, "enemy0 appears in ARR0 and ARR1");
        assert_eq!(rep.candidates, 7, "3 + 2 + 1 NPC + 1 non-NPC");
        assert_eq!(c.targets.len(), 6, "the non-NPC is screened out");
        assert_eq!(c.backend.read_rejections, 0, "all reads within 4096");
    }
}
//! SpeedForce.ini loader, validator and diagnostics.
//!
//! Design rules that this module must uphold (from the J task):
//!
//! 1. Only gameplay-facing values are configurable. Memory offsets, the
//!    identity-verification chain and the write backend are NOT reachable from
//!    the INI - the parser simply has no key for them, so a typo cannot become
//!    an offset.
//! 2. A value that fails validation produces an ERROR, never a silent fallback
//!    to something else. The caller turns errors into a halt.
//! 3. Every key that is parsed but not implemented yet is reported as
//!    NOT_IMPLEMENTED, so `Enabled=1` can never be a no-op that stays quiet.
//! 4. Precedence is env > ini > default. The env layer exists because the
//!    I radius scripts already drive the DLL through environment variables;
//!    keeping env on top means those scripts behave exactly as before.
//!
//! The parser is hand-written on purpose: this crate has zero dependencies and
//! a 150-line INI reader is cheaper to audit than a new dependency tree.

use std::fmt;

/// File name looked up next to the game executable when `SPEEDFORCE_INI`
/// is not set.
pub const DEFAULT_INI_FILE: &str = "SpeedForce.ini";

/// Hard limits. These mirror `core::Config::valid_*`; the numbers are duplicated
/// here so the INI layer can explain *why* something was rejected.
pub const RADIUS_MIN: f32 = 1.0;
pub const RADIUS_MAX: f32 = 200.0;
pub const SCALE_MIN_EXCLUSIVE: f32 = 0.0;
pub const SCALE_MAX: f32 = 1.0;
pub const DURATION_MIN_MS: u64 = 1000;
pub const DURATION_MAX_MS: u64 = 600_000;
pub const MAX_TARGETS_MIN: usize = 1;
/// the 神奇素 signal SpEffect. Confirmed live by the K probe: id 34,
/// duration 0.10 s, observed lifetime ~0.15 s.
pub const DEFAULT_SIGNAL_SPEFFECT_ID: i32 = 34;
/// Poll interval bounds. 5 ms is the floor: the game's own tick is ~16 ms, and a
/// 0.10 s signal still gives ~6 samples at 16 ms without busy-waiting.
pub const POLL_MIN_MS: u64 = 5;
pub const POLL_MAX_MS: u64 = 500;
pub const DEFAULT_POLL_MS: u64 = 10;
/// TARGET_LIMIT is only an *extra* cap; 0 means "no extra limit" and the
/// existing 128 hard ceiling still applies.
pub const TARGET_LIMIT_MAX: usize = MAX_TARGETS_MAX;
pub const MAX_TARGETS_MAX: usize = 128;

pub const DEFAULT_RADIUS: f32 = 80.0;
pub const DEFAULT_SCALE: f32 = 0.2;
pub const DEFAULT_DURATION_MS: u64 = 15_000;
pub const DEFAULT_MAX_TARGETS: usize = 128;

/// Where a resolved value came from. Printed once per key at startup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    Default,
    Ini,
    Env,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Source::Default => "default",
            Source::Ini => "ini",
            Source::Env => "env",
        })
    }
}

/// A parsed-but-unimplemented option. Every one of these is logged, so
/// `Enabled=1` in an unimplemented section is visible rather than silent.
#[derive(Clone, Debug)]
pub struct Reserved {
    pub section: &'static str,
    pub enabled: bool,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct Settings {
    // [General]
    pub enabled: bool,
    pub debug_log: bool,
    // [SlowMotion]
    pub radius_m: f32,
    pub slow_scale: f32,
    pub duration_ms: u64,
    pub max_targets: usize,
    // [Trigger]
    pub trigger_mode: String,
    pub hotkey_enabled: bool,
    pub hotkey: String,
    pub reject_while_active: bool,
    pub require_rising_edge: bool,
    /// poll the player's SpEffect chain for `item_signal_speffect_id`.
    pub item_trigger_live: bool,
    /// how often the worker polls, in ms (a lower bound - the loop tick
    /// may be slower).
    pub poll_interval_ms: u64,
    /// Extra per-activation cap, 0 = no extra limit (hard cap still applies).
    pub target_limit: usize,
    /// L Phase C/D boundary. 1 = detect and log the rising edge only;
    /// 0 = also call request_activation(Item). Default 1 so the first live run
    /// measures capture rate without silently changing what the mod does.
    pub item_dry_run: bool,
    // [ItemTrigger] / [SkillTrigger] - parsed, RESERVED
    pub item_enabled: bool,
    pub item_goods_id: i32,
    pub item_signal_speffect_id: i32,
    pub skill_enabled: bool,
    pub skill_weapon_id: i32,
    pub skill_sword_arts_id: i32,
    pub skill_signal_speffect_id: i32,
    // [FXR_*] / [Sound_*] - parsed, RESERVED
    pub fxr_startup_enabled: bool,
    pub fxr_startup_ids: Vec<i32>,
    pub fxr_loop_enabled: bool,
    pub fxr_loop_ids: Vec<i32>,
    pub fxr_end_enabled: bool,
    pub fxr_end_ids: Vec<i32>,
    pub sound_startup_enabled: bool,
    pub sound_startup_id: i32,
    pub sound_loop_enabled: bool,
    pub sound_loop_id: i32,
    pub sound_end_enabled: bool,
    pub sound_end_id: i32,

    // ---- diagnostics -------------------------------------------------------
    pub ini_path: String,
    pub ini_found: bool,
    /// (key, value, source) for every gameplay value actually in force.
    pub resolved: Vec<(String, String, Source)>,
    /// Fatal: the caller must halt instead of running with a surprise value.
    pub errors: Vec<String>,
    /// Non-fatal: value accepted, but the requested behaviour is not implemented.
    pub warnings: Vec<String>,
    pub reserved: Vec<Reserved>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false, // safe default: no ini -> disarmed
            debug_log: true,
            radius_m: DEFAULT_RADIUS,
            slow_scale: DEFAULT_SCALE,
            duration_ms: DEFAULT_DURATION_MS,
            max_targets: DEFAULT_MAX_TARGETS,
            trigger_mode: "DebugHotkey".to_string(),
            hotkey_enabled: true,
            hotkey: "F8".to_string(),
            reject_while_active: true,
            require_rising_edge: true,
            item_trigger_live: false,
            poll_interval_ms: DEFAULT_POLL_MS,
            target_limit: 0,
            item_dry_run: true,
            item_enabled: false,
            item_goods_id: 0,
            item_signal_speffect_id: 0,
            skill_enabled: false,
            skill_weapon_id: 0,
            skill_sword_arts_id: 0,
            skill_signal_speffect_id: 0,
            fxr_startup_enabled: false,
            fxr_startup_ids: Vec::new(),
            fxr_loop_enabled: false,
            fxr_loop_ids: Vec::new(),
            fxr_end_enabled: false,
            fxr_end_ids: Vec::new(),
            sound_startup_enabled: false,
            sound_startup_id: 0,
            sound_loop_enabled: false,
            sound_loop_id: 0,
            sound_end_enabled: false,
            sound_end_id: 0,
            ini_path: String::new(),
            ini_found: false,
            resolved: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            reserved: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// raw parsing
// ---------------------------------------------------------------------------

/// One `key = value` line with the section it belongs to. Section and key are
/// lower-cased so lookups are case-insensitive; values keep their case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub section: String,
    /// Lower-cased, used for lookup.
    pub key: String,
    /// As written by the user, used in messages so they read `Radius`, not
    /// `radius`, and can be found in the file with Ctrl+F.
    pub raw_key: String,
    pub value: String,
    pub line: usize,
}

/// Parse INI text. Never fails: malformed lines become entries in `problems`
/// and everything else is still returned, so one typo does not hide the rest.
pub fn parse(text: &str) -> (Vec<Entry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut problems = Vec::new();
    let mut section = String::new();

    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            match rest.find(']') {
                Some(end) => {
                    section = rest[..end].trim().to_ascii_lowercase();
                    // the task book writes [SpeedForce]; the shipped
                    // template uses [SlowMotion]. One set of gameplay keys, not
                    // two that can disagree.
                    if section == "speedforce" {
                        section = "slowmotion".to_string();
                    }
                    if section.is_empty() {
                        problems.push(format!("line {}: empty section name", line_no));
                    }
                }
                None => problems.push(format!("line {}: '[' without ']'", line_no)),
            }
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) => {
                let key = k.trim().to_ascii_lowercase();
                if key.is_empty() {
                    problems.push(format!("line {}: empty key", line_no));
                    continue;
                }
                if section.is_empty() {
                    problems.push(format!(
                        "line {}: key '{}' appears before any [section], ignored",
                        line_no, key
                    ));
                    continue;
                }
                entries.push(Entry {
                    section: section.clone(),
                    raw_key: k.trim().to_string(),
                    key,
                    value: v.trim().to_string(),
                    line: line_no,
                });
            }
            None => problems.push(format!("line {}: not 'key=value'", line_no)),
        }
    }
    (entries, problems)
}

/// `;` and `#` start a comment. Deliberately does not try to be clever about
/// quoting: no value in this file needs an embedded comment marker.
fn strip_comment(s: &str) -> &str {
    match s.find(|c| c == ';' || c == '#') {
        Some(i) => &s[..i],
        None => s,
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_u64(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok()
}

fn parse_i32(s: &str) -> Option<i32> {
    s.trim().parse::<i32>().ok()
}

/// Accepts `26`, `26,27`, `26 27`, `26, 27`.
fn parse_id_list(s: &str) -> Option<Vec<i32>> {
    let t = s.trim();
    if t.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for part in t.split(|c| c == ',' || c == ' ' || c == '\t') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p.parse::<i32>() {
            Ok(v) if v > 0 => out.push(v),
            _ => return None,
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// typed lookup with error reporting
// ---------------------------------------------------------------------------
struct Lookup<'a> {
    entries: &'a [Entry],
    errors: Vec<String>,
    warnings: Vec<String>,
}

impl<'a> Lookup<'a> {
    fn new(entries: &'a [Entry]) -> Self {
        Lookup { entries, errors: Vec::new(), warnings: Vec::new() }
    }

    /// Last occurrence wins; duplicates are reported because they are almost
    /// always an editing accident, not intent.
    fn last(&mut self, section: &str, key: &str) -> Option<&'a Entry> {
        let hits: Vec<&'a Entry> = self
            .entries
            .iter()
            .filter(|e| e.section == section && e.key == key)
            .collect();
        if hits.len() > 1 {
            self.warnings.push(format!(
                "[{}] {} appears {} times (lines {:?}); the last one wins",
                section,
                hits[0].raw_key,
                hits.len(),
                hits.iter().map(|e| e.line).collect::<Vec<_>>()
            ));
        }
        hits.last().copied()
    }

    fn boolean(&mut self, section: &str, key: &str, default: bool) -> bool {
        let Some(e) = self.last(section, key) else { return default };
        match parse_bool(&e.value) {
            Some(b) => b,
            None => {
                self.errors.push(format!(
                    "[{}] {} = '{}' is not a boolean (use 1 or 0)",
                    section, e.raw_key, e.value
                ));
                default
            }
        }
    }

    fn f32_range(
        &mut self,
        section: &str,
        key: &str,
        default: f32,
        lo: f32,
        hi: f32,
        exclusive_zero: bool,
    ) -> f32 {
        let Some(e) = self.last(section, key) else { return default };
        match parse_f32(&e.value) {
            Some(x) => {
                let ok = if exclusive_zero { x > lo && x <= hi } else { x >= lo && x <= hi };
                if ok {
                    x
                } else {
                    let bound = if exclusive_zero {
                        format!("{} < value <= {}", lo, hi)
                    } else {
                        format!("{} <= value <= {}", lo, hi)
                    };
                    self.errors.push(format!(
                        "[{}] {} = {} is out of range ({})",
                        section, e.raw_key, x, bound
                    ));
                    default
                }
            }
            None => {
                self.errors.push(format!(
                    "[{}] {} = '{}' is not a number",
                    section, e.raw_key, e.value
                ));
                default
            }
        }
    }

    fn u64_range(&mut self, section: &str, key: &str, default: u64, lo: u64, hi: u64) -> u64 {
        let Some(e) = self.last(section, key) else { return default };
        match parse_u64(&e.value) {
            Some(x) if x >= lo && x <= hi => x,
            Some(x) => {
                self.errors.push(format!(
                    "[{}] {} = {} is out of range ({} <= value <= {})",
                    section, e.raw_key, x, lo, hi
                ));
                default
            }
            None => {
                self.errors.push(format!(
                    "[{}] {} = '{}' is not an integer",
                    section, e.raw_key, e.value
                ));
                default
            }
        }
    }

    fn usize_range(
        &mut self,
        section: &str,
        key: &str,
        default: usize,
        lo: usize,
        hi: usize,
    ) -> usize {
        self.u64_range(section, key, default as u64, lo as u64, hi as u64) as usize
    }

    fn i32_val(&mut self, section: &str, key: &str, default: i32) -> i32 {
        let Some(e) = self.last(section, key) else { return default };
        match parse_i32(&e.value) {
            Some(x) => x,
            None => {
                self.errors.push(format!(
                    "[{}] {} = '{}' is not an integer",
                    section, e.raw_key, e.value
                ));
                default
            }
        }
    }

    fn ids(&mut self, section: &str, key: &str) -> Vec<i32> {
        let Some(e) = self.last(section, key) else { return Vec::new() };
        match parse_id_list(&e.value) {
            Some(list) => list,
            None => {
                self.errors.push(format!(
                    "[{}] {} = '{}' is not a list of positive ids",
                    section, e.raw_key, e.value
                ));
                Vec::new()
            }
        }
    }

    fn text(&mut self, section: &str, key: &str, default: &str) -> String {
        self.last(section, key)
            .map(|e| e.value.clone())
            .unwrap_or_else(|| default.to_string())
    }
}

// ---------------------------------------------------------------------------
// Settings construction
// ---------------------------------------------------------------------------

impl Settings {
    /// Build settings from INI text. `path` / `found` only feed diagnostics.
    pub fn from_ini_text(text: &str, path: &str, found: bool) -> Settings {
        let mut s = Settings { ini_path: path.to_string(), ini_found: found, ..Default::default() };
        let (entries, problems) = parse(text);
        for p in problems {
            s.warnings.push(format!("ini syntax: {}", p));
        }
        let mut lk = Lookup::new(&entries);

        // ---- [General]
        s.enabled = lk.boolean("general", "enabled", s.enabled);
        s.debug_log = lk.boolean("general", "debuglog", s.debug_log);

        // ---- [SlowMotion]
        s.radius_m = lk.f32_range("slowmotion", "radius", s.radius_m, RADIUS_MIN, RADIUS_MAX, false);
        s.slow_scale = lk.f32_range(
            "slowmotion", "slowscale", s.slow_scale,
            SCALE_MIN_EXCLUSIVE, SCALE_MAX, true,
        );
        // the task book calls this `Scale`; the template says `SlowScale`.
        // Accept both, same key, same range.
        s.slow_scale = lk.f32_range(
            "slowmotion", "scale", s.slow_scale,
            SCALE_MIN_EXCLUSIVE, SCALE_MAX, true,
        );
        s.duration_ms = lk.u64_range(
            "slowmotion", "durationms", s.duration_ms, DURATION_MIN_MS, DURATION_MAX_MS,
        );
        s.max_targets = lk.usize_range(
            "slowmotion", "maxtargets", s.max_targets, MAX_TARGETS_MIN, MAX_TARGETS_MAX,
        );

        // ---- [Trigger]
        s.trigger_mode = lk.text("trigger", "mode", &s.trigger_mode);
        s.hotkey_enabled = lk.boolean("trigger", "debughotkeyenabled", s.hotkey_enabled);
        s.hotkey = lk.text("trigger", "debughotkey", &s.hotkey);
        s.reject_while_active = lk.boolean("trigger", "rejectwhileactive", s.reject_while_active);
        s.require_rising_edge = lk.boolean("trigger", "requiresignalrisingedge", s.require_rising_edge);
        // EnableF8Debug is the task-book name for DebugHotkeyEnabled.
        s.hotkey_enabled = lk.boolean("trigger", "enablef8debug", s.hotkey_enabled);
        s.poll_interval_ms = lk.u64_range(
            "trigger", "pollintervalms", s.poll_interval_ms, POLL_MIN_MS, POLL_MAX_MS,
        );
        s.target_limit = lk.usize_range(
            "slowmotion", "targetlimit", s.target_limit, 0, TARGET_LIMIT_MAX,
        );

        // ---- [ItemTrigger] / [SkillTrigger]  (RESERVED)
        s.item_enabled = lk.boolean("itemtrigger", "enabled", s.item_enabled);
        s.item_goods_id = lk.i32_val("itemtrigger", "goodsid", s.item_goods_id);
        s.item_signal_speffect_id =
            lk.i32_val("itemtrigger", "signalspeffectid", s.item_signal_speffect_id);
        // the live key lives in [Trigger]; [ItemTrigger] stays as the
        // legacy location so an existing file keeps working.
        s.item_dry_run = lk.boolean("trigger", "itemtriggerdryrun", s.item_dry_run);
        s.item_trigger_live = lk.boolean("trigger", "enableitemtrigger", s.item_enabled);
        s.item_enabled = s.item_trigger_live;
        let legacy_sig = s.item_signal_speffect_id;
        s.item_signal_speffect_id = lk.i32_val(
            "trigger",
            "signalspeffectid",
            if legacy_sig != 0 { legacy_sig } else { DEFAULT_SIGNAL_SPEFFECT_ID },
        );
        s.skill_enabled = lk.boolean("skilltrigger", "enabled", s.skill_enabled);
        s.skill_weapon_id = lk.i32_val("skilltrigger", "weaponid", s.skill_weapon_id);
        s.skill_sword_arts_id = lk.i32_val("skilltrigger", "swordartsid", s.skill_sword_arts_id);
        s.skill_signal_speffect_id =
            lk.i32_val("skilltrigger", "signalspeffectid", s.skill_signal_speffect_id);

        // ---- [FXR_*] / [Sound_*]  (RESERVED)
        s.fxr_startup_enabled = lk.boolean("fxr_startup", "enabled", s.fxr_startup_enabled);
        s.fxr_startup_ids = lk.ids("fxr_startup", "fxr_ids");
        s.fxr_loop_enabled = lk.boolean("fxr_loop", "enabled", s.fxr_loop_enabled);
        s.fxr_loop_ids = lk.ids("fxr_loop", "fxr_ids");
        s.fxr_end_enabled = lk.boolean("fxr_end", "enabled", s.fxr_end_enabled);
        s.fxr_end_ids = lk.ids("fxr_end", "fxr_ids");
        s.sound_startup_enabled = lk.boolean("sound_startup", "enabled", s.sound_startup_enabled);
        s.sound_startup_id = lk.i32_val("sound_startup", "soundeventid", s.sound_startup_id);
        s.sound_loop_enabled = lk.boolean("sound_loop", "enabled", s.sound_loop_enabled);
        s.sound_loop_id = lk.i32_val("sound_loop", "soundeventid", s.sound_loop_id);
        s.sound_end_enabled = lk.boolean("sound_end", "enabled", s.sound_end_enabled);
        s.sound_end_id = lk.i32_val("sound_end", "soundeventid", s.sound_end_id);

        s.errors.append(&mut lk.errors);
        s.warnings.append(&mut lk.warnings);

        // ---- semantics that only have one implemented behaviour ------------
        if !s.reject_while_active {
            s.warnings.push(
                "RejectWhileActive=0 is NOT_IMPLEMENTED; forced to 1 (Active always rejects)"
                    .to_string(),
            );
            s.reject_while_active = true;
        }
        if !s.require_rising_edge {
            s.warnings.push(
                "RequireSignalRisingEdge=0 is NOT_IMPLEMENTED; forced to 1 (level trigger would \
                 re-fire every frame)"
                    .to_string(),
            );
            s.require_rising_edge = true;
        }
        if !s.hotkey.eq_ignore_ascii_case("f8") {
            s.warnings.push(format!(
                "DebugHotkey='{}' is NOT_IMPLEMENTED; only F8 is wired. Hotkey disabled.",
                s.hotkey
            ));
            s.hotkey_enabled = false;
        }
        if s.item_trigger_live && s.item_dry_run {
            s.warnings.push(
                "ItemTriggerDryRun=1: the detector will log ITEM_SIGNAL_RISING_EDGE but will \
                 NOT start the slowdown. Set it to 0 once capture rate is confirmed."
                    .to_string(),
            );
        }
        if s.item_trigger_live && s.item_signal_speffect_id <= 0 {
            s.errors.push(
                "[Trigger] EnableItemTrigger=1 but SignalSpEffectID is not a positive id"
                    .to_string(),
            );
        }
        if s.item_trigger_live && !s.enabled {
            s.warnings.push(
                "EnableItemTrigger=1 but [General] Enabled=0: the detector will poll but the \
                 slowdown core stays disarmed"
                    .to_string(),
            );
        }
        if s.enabled && !s.ini_found {
            s.warnings.push(
                "ini not found: Enabled defaults to 0 (disarmed) on purpose - run with \
                 SPEEDFORCE_M4E_ARM=1 or create SpeedForce.ini"
                    .to_string(),
            );
        }

        // ---- RESERVED reporting -------------------------------------------
        // ItemTrigger is now IMPLEMENTED, so it must NOT be reported as
        // RESERVED any more. GoodsID stays diagnostic-only (it never gates the
        // trigger; the SpEffect id does).
        if s.skill_enabled || s.skill_weapon_id != 0
            || s.skill_sword_arts_id != 0 || s.skill_signal_speffect_id != 0
        {
            s.reserved.push(Reserved {
                section: "SkillTrigger",
                enabled: s.skill_enabled,
                detail: format!(
                    "WeaponID={} SwordArtsID={} SignalSpEffectID={} -> RESERVED / \
                     NOT_IMPLEMENTED, no effect",
                    s.skill_weapon_id, s.skill_sword_arts_id, s.skill_signal_speffect_id
                ),
            });
        }
        for (name, on, ids, kind) in [
            ("FXR_Startup", s.fxr_startup_enabled, &s.fxr_startup_ids, "fxr"),
            ("FXR_Loop", s.fxr_loop_enabled, &s.fxr_loop_ids, "fxr"),
            ("FXR_End", s.fxr_end_enabled, &s.fxr_end_ids, "fxr"),
        ] {
            if on || !ids.is_empty() {
                s.reserved.push(Reserved {
                    section: name,
                    enabled: on,
                    detail: format!("{} ids={:?} -> RESERVED / NOT_IMPLEMENTED", kind, ids),
                });
            }
        }
        for (name, on, id, kind) in [
            ("Sound_Startup", s.sound_startup_enabled, s.sound_startup_id, "sound"),
            ("Sound_Loop", s.sound_loop_enabled, s.sound_loop_id, "sound"),
            ("Sound_End", s.sound_end_enabled, s.sound_end_id, "sound"),
        ] {
            if on || id != 0 {
                s.reserved.push(Reserved {
                    section: name,
                    enabled: on,
                    detail: format!("{} id={} -> RESERVED / NOT_IMPLEMENTED", kind, id),
                });
            }
        }

        s.resolved.push((
            "Enabled".into(),
            format!("{}", s.enabled as u8),
            if found { Source::Ini } else { Source::Default },
        ));
        s.resolved.push((
            "DebugLog".into(),
            format!("{}", s.debug_log as u8),
            if found { Source::Ini } else { Source::Default },
        ));
        for (k, v) in [
            ("Radius", format!("{}", s.radius_m)),
            ("SlowScale", format!("{}", s.slow_scale)),
            ("DurationMs", format!("{}", s.duration_ms)),
            ("MaxTargets", format!("{}", s.max_targets)),
            ("TargetLimit", format!("{}", s.target_limit)),
            ("DebugHotkeyEnabled", format!("{}", s.hotkey_enabled as u8)),
            ("EnableItemTrigger", format!("{}", s.item_trigger_live as u8)),
            ("SignalSpEffectID", format!("{}", s.item_signal_speffect_id)),
            ("PollIntervalMs", format!("{}", s.poll_interval_ms)),
            ("ItemTriggerDryRun", format!("{}", s.item_dry_run as u8)),
        ] {
            s.resolved.push((k.into(), v, if found { Source::Ini } else { Source::Default }));
        }
        s
    }

    /// Overlay environment variables. Env wins over INI, which keeps every
    /// existing I script working unchanged.
    pub fn apply_env_overrides<F>(&mut self, get: F)
    where
        F: Fn(&str) -> Option<String>,
    {
        if let Some(v) = get("SPEEDFORCE_M4E_ARM") {
            let on = v.trim() == "1";
            self.enabled = on;
            self.resolved.push(("Enabled".into(), format!("{}", on as u8), Source::Env));
        }
        if let Some(v) = get("SPEEDFORCE_RADIUS") {
            if let Some(x) = parse_f32(&v) {
                if x >= RADIUS_MIN && x <= RADIUS_MAX {
                    self.radius_m = x;
                    self.resolved.push(("Radius".into(), format!("{}", x), Source::Env));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_RADIUS={} out of range ({}..={})",
                        x, RADIUS_MIN, RADIUS_MAX
                    ));
                }
            } else {
                self.errors
                    .push(format!("env SPEEDFORCE_RADIUS='{}' is not a number", v));
            }
        }
        if let Some(v) = get("SPEEDFORCE_M4E_SCALE") {
            if let Some(x) = parse_f32(&v) {
                if x > SCALE_MIN_EXCLUSIVE && x <= SCALE_MAX {
                    self.slow_scale = x;
                    self.resolved.push(("SlowScale".into(), format!("{}", x), Source::Env));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_M4E_SCALE={} out of range (0 < x <= 1)",
                        x
                    ));
                }
            } else {
                self.errors.push(format!("env SPEEDFORCE_M4E_SCALE='{}' is not a number", v));
            }
        }
        if let Some(v) = get("SPEEDFORCE_M4E_DURATION_MS") {
            if let Some(x) = parse_u64(&v) {
                if x >= DURATION_MIN_MS && x <= DURATION_MAX_MS {
                    self.duration_ms = x;
                    self.resolved
                        .push(("DurationMs".into(), format!("{}", x), Source::Env));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_M4E_DURATION_MS={} out of range ({}..={})",
                        x, DURATION_MIN_MS, DURATION_MAX_MS
                    ));
                }
            } else {
                self.errors.push(format!(
                    "env SPEEDFORCE_M4E_DURATION_MS='{}' is not an integer",
                    v
                ));
            }
        }
        if let Some(v) = get("SPEEDFORCE_M4F_MAX_TARGETS") {
            if let Some(x) = parse_u64(&v) {
                if x >= MAX_TARGETS_MIN as u64 && x <= MAX_TARGETS_MAX as u64 {
                    self.max_targets = x as usize;
                    self.resolved
                        .push(("MaxTargets".into(), format!("{}", x), Source::Env));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_M4F_MAX_TARGETS={} out of range ({}..={})",
                        x, MAX_TARGETS_MIN, MAX_TARGETS_MAX
                    ));
                }
            } else {
                self.errors.push(format!(
                    "env SPEEDFORCE_M4F_MAX_TARGETS='{}' is not an integer",
                    v
                ));
            }
        }
        if let Some(v) = get("SPEEDFORCE_M4F_TARGET_LIMIT") {
            if let Some(x) = parse_u64(&v) {
                if x <= TARGET_LIMIT_MAX as u64 {
                    self.target_limit = x as usize;
                    self.resolved
                        .push(("TargetLimit".into(), format!("{}", x), Source::Env));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_M4F_TARGET_LIMIT={} out of range (0..={})",
                        x, TARGET_LIMIT_MAX
                    ));
                }
            } else {
                self.errors.push(format!(
                    "env SPEEDFORCE_M4F_TARGET_LIMIT='{}' is not an integer",
                    v
                ));
            }
        }
        if let Some(v) = get("SPEEDFORCE_ITEM_TRIGGER") {
            if let Some(b) = parse_bool(&v) {
                self.item_trigger_live = b;
                self.item_enabled = b;
                self.resolved
                    .push(("EnableItemTrigger".into(), format!("{}", b as u8), Source::Env));
            } else {
                self.errors
                    .push(format!("env SPEEDFORCE_ITEM_TRIGGER='{}' is not a boolean", v));
            }
        }
        if let Some(v) = get("SPEEDFORCE_SIGNAL_SPEFFECT_ID") {
            if let Some(x) = parse_i32(&v) {
                if x > 0 {
                    self.item_signal_speffect_id = x;
                    self.resolved.push((
                        "SignalSpEffectID".into(),
                        format!("{}", x),
                        Source::Env,
                    ));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_SIGNAL_SPEFFECT_ID={} must be positive", x
                    ));
                }
            } else {
                self.errors.push(format!(
                    "env SPEEDFORCE_SIGNAL_SPEFFECT_ID='{}' is not an integer", v
                ));
            }
        }
        if let Some(v) = get("SPEEDFORCE_POLL_MS") {
            if let Some(x) = parse_u64(&v) {
                if x >= POLL_MIN_MS && x <= POLL_MAX_MS {
                    self.poll_interval_ms = x;
                    self.resolved
                        .push(("PollIntervalMs".into(), format!("{}", x), Source::Env));
                } else {
                    self.errors.push(format!(
                        "env SPEEDFORCE_POLL_MS={} out of range ({}..={})",
                        x, POLL_MIN_MS, POLL_MAX_MS
                    ));
                }
            } else {
                self.errors
                    .push(format!("env SPEEDFORCE_POLL_MS='{}' is not an integer", v));
            }
        }
        if let Some(v) = get("SPEEDFORCE_HOTKEY_ENABLED") {
            if let Some(b) = parse_bool(&v) {
                self.hotkey_enabled = b;
                self.resolved.push((
                    "DebugHotkeyEnabled".into(),
                    format!("{}", b as u8),
                    Source::Env,
                ));
            } else {
                self.errors.push(format!(
                    "env SPEEDFORCE_HOTKEY_ENABLED='{}' is not a boolean",
                    v
                ));
            }
        }
    }

    /// One-line summary for the log, listing only the values that matter.
    pub fn summary(&self, source_of: impl Fn(&str) -> &'static str) -> String {
        format!(
            "ini={} found={} Enabled={} Radius={} SlowScale={} DurationMs={} MaxTargets={} \
             Hotkey={} HotkeyEnabled={} src(radius)={} src(scale)={}",
            if self.ini_path.is_empty() { "<none>" } else { &self.ini_path },
            self.ini_found,
            self.enabled as u8,
            self.radius_m,
            self.slow_scale,
            self.duration_ms,
            self.max_targets,
            self.hotkey,
            self.hotkey_enabled as u8,
            source_of("Radius"),
            source_of("SlowScale"),
        )
    }

    /// The last source recorded for `key` (env overrode ini overrode default).
    pub fn source(&self, key: &str) -> Source {
        self.resolved
            .iter()
            .rev()
            .find(|(k, _, _)| k == key)
            .map(|(_, _, s)| *s)
            .unwrap_or(Source::Default)
    }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[General]
Enabled=1
DebugLog=0

[SlowMotion]
Radius=80
SlowScale=0.35
DurationMs=12000
MaxTargets=64

[Trigger]
Mode=DebugHotkey
DebugHotkeyEnabled=0
DebugHotkey=F8
RejectWhileActive=1
RequireSignalRisingEdge=1

[ItemTrigger]
Enabled=1
GoodsID=1234567
SignalSpEffectID=7654321

[FXR_Startup]
Enabled=1
FXR_IDs=26
"#;

    #[test]
    fn c1_parses_every_implemented_value() {
        let s = Settings::from_ini_text(SAMPLE, "SpeedForce.ini", true);
        assert!(s.errors.is_empty(), "unexpected errors: {:?}", s.errors);
        assert!(s.enabled);
        assert!(!s.debug_log);
        assert!((s.radius_m - 80.0).abs() < 1e-6);
        assert!((s.slow_scale - 0.35).abs() < 1e-6);
        assert_eq!(s.duration_ms, 12_000);
        assert_eq!(s.max_targets, 64);
        assert!(!s.hotkey_enabled);
        assert_eq!(s.source("Radius"), Source::Ini);
    }

    #[test]
    fn c2_out_of_range_radius_is_an_error_not_a_fallback() {
        let s = Settings::from_ini_text("[SlowMotion]\nRadius=999\n", "x", true);
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].contains("Radius"), "{:?}", s.errors);
        assert!(s.errors[0].contains("out of range"));
    }

    #[test]
    fn c3_zero_scale_is_rejected() {
        let s = Settings::from_ini_text("[SlowMotion]\nSlowScale=0\n", "x", true);
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].contains("SlowScale"));
    }

    #[test]
    fn c4_non_numeric_value_is_an_error() {
        let s = Settings::from_ini_text("[SlowMotion]\nDurationMs=abc\n", "x", true);
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].contains("not an integer"));
    }

    #[test]
    fn c5_missing_ini_defaults_to_disarmed() {
        let s = Settings::from_ini_text("", "SpeedForce.ini", false);
        assert!(!s.enabled, "no ini must not silently arm the mod");
        assert_eq!(s.radius_m, DEFAULT_RADIUS);
        assert_eq!(s.source("Radius"), Source::Default);
    }

    #[test]
    fn c6_item_trigger_is_implemented_so_it_is_not_reserved() {
        // [ItemTrigger] moved from RESERVED to real. Reporting it as
        // NOT_IMPLEMENTED now would be a lie in the exact place the operator
        // looks to find out what works.
        let s = Settings::from_ini_text(SAMPLE, "x", true);
        let names: Vec<&str> = s.reserved.iter().map(|r| r.section).collect();
        assert!(!names.contains(&"ItemTrigger"), "reserved: {:?}", names);
        assert!(names.contains(&"FXR_Startup"), "still unimplemented");
        assert!(s.item_trigger_live);
        // legacy [ItemTrigger] SignalSpEffectID is honoured when [Trigger] has none
        assert_eq!(s.item_signal_speffect_id, 7_654_321);
    }

    #[test]
    fn c13_signal_keys_and_poll_interval() {
        let s = Settings::from_ini_text(
            "[Trigger]\nEnableItemTrigger=1\nSignalSpEffectID=34\nPollIntervalMs=12\n",
            "x",
            true,
        );
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!(s.item_trigger_live);
        assert_eq!(s.item_signal_speffect_id, 34);
        assert_eq!(s.poll_interval_ms, 12);
    }

    #[test]
    fn c14_poll_interval_is_bounded() {
        let too_fast = Settings::from_ini_text("[Trigger]\nPollIntervalMs=0\n", "x", true);
        assert_eq!(too_fast.errors.len(), 1, "0 ms must be rejected, not clamped");
        let too_slow = Settings::from_ini_text("[Trigger]\nPollIntervalMs=5000\n", "x", true);
        assert_eq!(too_slow.errors.len(), 1);
        assert_eq!(too_fast.poll_interval_ms, DEFAULT_POLL_MS, "falls back to default");
    }

    #[test]
    fn c15_speedforce_section_is_an_alias_for_slowmotion() {
        // The L task book writes [SpeedForce]; the shipped template uses
        // [SlowMotion]. Both must drive the same keys.
        let s = Settings::from_ini_text(
            "[SpeedForce]\nRadius=55\nScale=0.3\nDurationMs=9000\nTargetLimit=7\n",
            "x",
            true,
        );
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert!((s.radius_m - 55.0).abs() < 1e-6);
        assert!((s.slow_scale - 0.3).abs() < 1e-6);
        assert_eq!(s.duration_ms, 9_000);
        assert_eq!(s.target_limit, 7);
    }

    #[test]
    fn c16_enable_f8_debug_alias() {
        let s = Settings::from_ini_text(
            "[Trigger]\nDebugHotkeyEnabled=1\nEnableF8Debug=0\n", "x", true);
        assert!(!s.hotkey_enabled, "EnableF8Debug wins when both are present");
        let s2 = Settings::from_ini_text("[Trigger]\nEnableF8Debug=1\n", "x", true);
        assert!(s2.hotkey_enabled);
    }

    #[test]
    fn c17_item_trigger_without_valid_signal_is_an_error() {
        let s = Settings::from_ini_text(
            "[Trigger]\nEnableItemTrigger=1\nSignalSpEffectID=0\n", "x", true);
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].contains("SignalSpEffectID"));
    }

    #[test]
    fn c18_target_limit_defaults_to_zero_and_is_bounded() {
        let s = Settings::from_ini_text("[SlowMotion]\nRadius=80\n", "x", true);
        assert_eq!(s.target_limit, 0, "0 = no extra limit");
        let bad = Settings::from_ini_text("[SlowMotion]\nTargetLimit=999\n", "x", true);
        assert_eq!(bad.errors.len(), 1);
    }

    #[test]
    fn c7_env_overrides_ini() {
        let mut s = Settings::from_ini_text("[SlowMotion]\nRadius=80\n", "x", true);
        assert!((s.radius_m - 80.0).abs() < 1e-6);
        s.apply_env_overrides(|n| match n {
            "SPEEDFORCE_RADIUS" => Some("45".to_string()),
            _ => None,
        });
        assert!((s.radius_m - 45.0).abs() < 1e-6);
        assert_eq!(s.source("Radius"), Source::Env);
        assert!(s.errors.is_empty());
    }

    #[test]
    fn c8_env_out_of_range_is_an_error() {
        let mut s = Settings::default();
        s.apply_env_overrides(|n| match n {
            "SPEEDFORCE_M4F_MAX_TARGETS" => Some("999".to_string()),
            _ => None,
        });
        assert_eq!(s.errors.len(), 1);
        assert!(s.errors[0].contains("MAX_TARGETS"));
    }

    #[test]
    fn c9_unimplemented_trigger_semantics_are_forced_and_reported() {
        let s = Settings::from_ini_text(
            "[Trigger]\nRejectWhileActive=0\nRequireSignalRisingEdge=0\nDebugHotkey=F9\n",
            "x",
            true,
        );
        assert!(s.reject_while_active, "forced to the only implemented behaviour");
        assert!(s.require_rising_edge);
        assert!(!s.hotkey_enabled, "unsupported key must disable the hotkey");
        assert!(s.warnings.iter().any(|w| w.contains("DebugHotkey='F9'")));
        assert!(s.warnings.iter().any(|w| w.contains("RejectWhileActive")));
    }

    #[test]
    fn c10_malformed_lines_do_not_hide_the_rest() {
        let s = Settings::from_ini_text(
            "garbage line\nOrphan=1\n[SlowMotion]\nRadius=50\nNoEquals\n",
            "x",
            true,
        );
        assert!((s.radius_m - 50.0).abs() < 1e-6, "valid key still applied");
        assert!(s.warnings.iter().any(|w| w.contains("not 'key=value'")));
        assert!(s.warnings.iter().any(|w| w.contains("before any [section]")));
    }

    #[test]
    fn c11_duplicate_key_warns_and_last_wins() {
        let s = Settings::from_ini_text("[SlowMotion]\nRadius=40\nRadius=60\n", "x", true);
        assert!((s.radius_m - 60.0).abs() < 1e-6);
        assert!(s.warnings.iter().any(|w| w.contains("appears 2 times")));
    }

    #[test]
    fn c12_id_list_forms() {
        let s = Settings::from_ini_text(
            "[FXR_Loop]\nFXR_IDs=26, 27 28\n", "x", true);
        assert_eq!(s.fxr_loop_ids, vec![26, 27, 28]);
        let s2 = Settings::from_ini_text("[FXR_Loop]\nFXR_IDs=\n", "x", true);
        assert!(s2.fxr_loop_ids.is_empty());
        assert!(s2.errors.is_empty());
    }
}

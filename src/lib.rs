//! P3 build: P1 core with chunked ChrArray reads (24000 -> 6 x 4096).
//!
//! What J adds on top of //!   * `SpeedForce.ini` next to the game exe (or `SPEEDFORCE_INI`) supplies
//!     Radius / SlowScale / DurationMs / MaxTargets / DebugHotkeyEnabled.
//!   * env > ini > default precedence, so every I radius script keeps
//!     behaving exactly as before.
//!   * every rejected value halts instead of silently running with something
//!     else, and every parsed-but-unimplemented section is logged as
//!     NOT_IMPLEMENTED.
//!
//! The slowdown core itself (identity chain, per-target write, rollback,
//! restore) is byte-for-byte the H logic: J does not touch it.
//!
//! F with a real world-position filter on top. Target selection is now:
//!   cheap identity screen -> CSChrPhysicsModule position -> squared distance
//!   <= SPEEDFORCE_RADIUS^2 -> full identity/behavior verification.
//! Only characters inside the radius ever reach the BehaviorModule checks.
//!
//! F8 is START-ONLY in this build: a press while Active is ignored, so the
//! 15 s window can never be cancelled by an accidental keypress.
//!
//! Windows backend notes:
//!   * All reads go through ONE `ReadProcessMemory` per contiguous block
//!     (ChrIns header = 1 call), instead of `VirtualQuery` + raw deref per
//!     field. That was the freeze: 11+ kernel calls per target, ~250 per F8.
//!   * Writes use `WriteProcessMemory` (4 bytes, checked). Same field and same
//!     value as E; the API just refuses instead of faulting.
//!   * `VirtualQuery` is now used ONLY by `writable()`, a handful of calls per
//!     activation, never per field.
//!   * `flush_events` batches one tick's events into a single write.
//!
//! Hard gates unchanged: `SPEEDFORCE_M4E_ARM=1`, identity snapshot per target,
//! write-forbidden on unknown objects, rollback checked, finalizer joins worker.

pub mod core;
pub mod config_file;
pub mod sp_signal;

#[cfg(not(test))]
mod win {
    use crate::config_file;
    use crate::sp_signal;
    use crate::core::{
        Backend, Config, Controller, RestoreOutcome, TriggerSource,
        DEFAULT_MAX_LIST_NODES, PerfCounters, MAX_CANDIDATE_SCAN,
        DEFAULT_PLAYER_OFF, DEFAULT_SPEED_OFFSET, DEFAULT_WCM_RVA, TICK_MS, VK_F8, VK_F9, VK_F10,
    };
    use std::ffi::c_void;
    use std::mem::{size_of, MaybeUninit};
    use std::ptr;
    use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU64, Ordering};

    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(v_key: i32) -> i16;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetTickCount64() -> u64;
        fn Sleep(ms: u32);
        fn CreateThread(
            attr: *mut c_void,
            stack: usize,
            start: extern "system" fn(*mut c_void) -> u32,
            param: *mut c_void,
            flags: u32,
            tid: *mut u32,
        ) -> *mut c_void;
        fn DisableThreadLibraryCalls(module: *mut c_void) -> i32;
        fn VirtualQuery(addr: *const c_void, buf: *mut c_void, len: usize) -> usize;
        fn CloseHandle(h: *mut c_void) -> i32;
        fn WaitForSingleObject(h: *mut c_void, milliseconds: u32) -> u32;
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn GetModuleFileNameW(module: *mut c_void, buf: *mut u16, len: u32) -> u32;
        fn GetCurrentProcess() -> *mut c_void;
        fn GetCurrentThread() -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            created: *mut FileTime,
            exited: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn ReadProcessMemory(
            process: *mut c_void,
            base: *const c_void,
            buffer: *mut c_void,
            size: usize,
            read: *mut usize,
        ) -> i32;
        fn WriteProcessMemory(
            process: *mut c_void,
            base: *mut c_void,
            buffer: *const c_void,
            size: usize,
            written: *mut usize,
        ) -> i32;
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn as_u64(self) -> u64 {
            ((self.high as u64) << 32) | self.low as u64
        }
    }

    // ---- per-primitive cost counters (nanoseconds, wall clock) ------------
    static RPM_N: AtomicU64 = AtomicU64::new(0);
    static RPM_NS: AtomicU64 = AtomicU64::new(0);
    static RPM_MAX: AtomicU64 = AtomicU64::new(0);
    static VQ_N: AtomicU64 = AtomicU64::new(0);
    static VQ_NS: AtomicU64 = AtomicU64::new(0);
    static VQ_MAX: AtomicU64 = AtomicU64::new(0);
    static WPM_N: AtomicU64 = AtomicU64::new(0);
    static WPM_NS: AtomicU64 = AtomicU64::new(0);
    static WPM_MAX: AtomicU64 = AtomicU64::new(0);

    #[inline]
    fn bump(n: &AtomicU64, sum: &AtomicU64, mx: &AtomicU64, spent: u64) {
        n.fetch_add(1, Ordering::Relaxed);
        sum.fetch_add(spent, Ordering::Relaxed);
        mx.fetch_max(spent, Ordering::Relaxed);
    }

    /// Kernel + user time this thread has consumed, in nanoseconds.
    fn thread_cpu_ns() -> u64 {
        unsafe {
            let mut c = FileTime::default();
            let mut e = FileTime::default();
            let mut k = FileTime::default();
            let mut u = FileTime::default();
            if GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) == 0 {
                return 0;
            }
            (k.as_u64() + u.as_u64()) * 100
        }
    }

    const DLL_PROCESS_ATTACH: u32 = 1;
    const DLL_PROCESS_DETACH: u32 = 0;
    const MEM_COMMIT: u32 = 0x1000;
    const WRITABLE_PROTECT: &[u32] = &[0x04, 0x08, 0x40, 0x80];
    // single source of truth lives in core.rs, shared with the Mock.
    // Two independent constants is exactly how the 24000-byte bug stayed
    // invisible to 69 offline tests.
    use crate::core::MAX_READ_BLOCK;

    // 0 NOT_INITIALIZED / 1 WORKER_RUNNING / 2 NOT_ACTIVE / 3 RESTORED /
    // 4 RESTORE_FAILED / 5 WRITE_FORBIDDEN / 6 FINALIZER_TIMEOUT /
    // 7 DETACH_WITHOUT_FINALIZER
    static STOP: AtomicBool = AtomicBool::new(false);
    static EXIT_STATUS: AtomicI32 = AtomicI32::new(0);
    static WORKER_HANDLE: AtomicIsize = AtomicIsize::new(0);
    /// This DLL's own HMODULE, captured in DllMain. Used to resolve the
    /// deployment directory without guessing or reading a config path.
    static DLL_HINST: AtomicIsize = AtomicIsize::new(0);
    static FINALIZER_CALLED: AtomicBool = AtomicBool::new(false);
    const FINALIZER_WAIT_MS: u32 = 3000;

    #[repr(C)]
    struct MemoryBasicInformation {
        base_address: usize,
        allocation_base: usize,
        allocation_protect: u32,
        pad1: u32,
        region_size: usize,
        state: u32,
        protect: u32,
        ty: u32,
        pad2: u32,
    }

    /// Used ONLY by `writable()`. Never called per field any more.
    fn page_writable(addr: usize) -> bool {
        if addr == 0 {
            return false;
        }
        let mut info = MaybeUninit::<MemoryBasicInformation>::zeroed();
        let t0 = std::time::Instant::now();
        let n = unsafe {
            VirtualQuery(
                addr as *const c_void,
                info.as_mut_ptr() as *mut c_void,
                size_of::<MemoryBasicInformation>(),
            )
        };
        bump(&VQ_N, &VQ_NS, &VQ_MAX, t0.elapsed().as_nanos() as u64);
        if n == 0 {
            return false;
        }
        let i = unsafe { info.assume_init() };
        i.state == MEM_COMMIT && WRITABLE_PROTECT.contains(&(i.protect & 0xFF))
    }

    struct WinBackend {
        static_player_ptr: usize,
        target_file: Option<String>,
        /// Measured on this machine: `WriteProcessMemory` costs ~56 ms of CPU
        /// per 4-byte write while `ReadProcessMemory` costs 0.8 us. We are
        /// inside the target process, so a direct volatile store is both
        /// correct and ~5 orders of magnitude cheaper. `SPEEDFORCE_SLOW_WRITE=1`
        /// restores the old API path for comparison.
        slow_write: bool,
    }

    impl Backend for WinBackend {
        fn now_ms(&self) -> u64 {
            unsafe { GetTickCount64() }
        }
        fn key_down(&self) -> bool {
            unsafe { (GetAsyncKeyState(VK_F8) as u16 & 0x8000) != 0 }
        }
        fn sweep_key(&self) -> bool {
            unsafe { (GetAsyncKeyState(VK_F9) as u16 & 0x8000) != 0 }
        }
        fn probe_key(&self) -> bool {
            unsafe { (GetAsyncKeyState(VK_F10) as u16 & 0x8000) != 0 }
        }
        fn target_ptr(&mut self) -> usize {
            match &self.target_file {
                Some(path) => match std::fs::read_to_string(path) {
                    Ok(s) => hex(s.trim()).unwrap_or(0),
                    Err(_) => 0,
                },
                None => self.static_player_ptr,
            }
        }
        fn module_base(&mut self) -> usize {
            unsafe { GetModuleHandleW(ptr::null()) as usize }
        }
        fn read_bytes(&mut self, addr: usize, len: usize) -> Option<Vec<u8>> {
            if addr == 0 || len == 0 || len > MAX_READ_BLOCK {
                return None;
            }
            let mut buf = vec![0u8; len];
            let mut got = 0usize;
            let t0 = std::time::Instant::now();
            let ok = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    addr as *const c_void,
                    buf.as_mut_ptr() as *mut c_void,
                    len,
                    &mut got,
                )
            };
            bump(&RPM_N, &RPM_NS, &RPM_MAX, t0.elapsed().as_nanos() as u64);
            if ok == 0 || got != len {
                None
            } else {
                Some(buf)
            }
        }
        fn write_f32(&mut self, addr: usize, value: f32) -> Result<(), ()> {
            if addr == 0 {
                return Err(());
            }
            if !self.slow_write {
                // In-process store. The caller has already read this exact
                // address (proving it is mapped) and re-reads it afterwards to
                // confirm the value landed, so a failed write is still caught.
                let t0 = std::time::Instant::now();
                unsafe { std::ptr::write_volatile(addr as *mut f32, value) };
                bump(&WPM_N, &WPM_NS, &WPM_MAX, t0.elapsed().as_nanos() as u64);
                return Ok(());
            }
            let bytes = value.to_le_bytes();
            let mut written = 0usize;
            let t0 = std::time::Instant::now();
            let ok = unsafe {
                WriteProcessMemory(
                    GetCurrentProcess(),
                    addr as *mut c_void,
                    bytes.as_ptr() as *const c_void,
                    4,
                    &mut written,
                )
            };
            bump(&WPM_N, &WPM_NS, &WPM_MAX, t0.elapsed().as_nanos() as u64);
            if ok == 0 || written != 4 {
                Err(())
            } else {
                Ok(())
            }
        }
        fn writable(&self, addr: usize) -> bool {
            page_writable(addr)
        }
        fn perf(&self) -> PerfCounters {
            PerfCounters {
                rpm_n: RPM_N.load(Ordering::Relaxed),
                rpm_ns: RPM_NS.load(Ordering::Relaxed),
                rpm_max_ns: RPM_MAX.load(Ordering::Relaxed),
                vq_n: VQ_N.load(Ordering::Relaxed),
                vq_ns: VQ_NS.load(Ordering::Relaxed),
                vq_max_ns: VQ_MAX.load(Ordering::Relaxed),
                wpm_n: WPM_N.load(Ordering::Relaxed),
                wpm_ns: WPM_NS.load(Ordering::Relaxed),
                wpm_max_ns: WPM_MAX.load(Ordering::Relaxed),
            }
        }
        fn reset_perf(&self) {
            for a in [&RPM_N, &RPM_NS, &RPM_MAX, &VQ_N, &VQ_NS, &VQ_MAX,
                      &WPM_N, &WPM_NS, &WPM_MAX] {
                a.store(0, Ordering::Relaxed);
            }
        }
        fn thread_cpu_ns(&self) -> u64 {
            thread_cpu_ns()
        }
    }

    fn env(name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
    fn hex(s: &str) -> Option<usize> {
        let t = s.trim();
        let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
        usize::from_str_radix(t, 16).ok()
    }
    fn dec_usize(s: &str, default: usize) -> usize {
        s.trim().parse::<usize>().unwrap_or(default)
    }
    fn exe_dir() -> std::path::PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    }

    /// Directory this DLL was loaded from, i.e. wherever `natives\SpeedForce.dll`
    /// actually sits. `None` only if the module handle was never captured.
    fn dll_dir() -> Option<std::path::PathBuf> {
        let h = DLL_HINST.load(Ordering::SeqCst) as *mut c_void;
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let n = unsafe { GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32) };
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..n as usize]);
        std::path::Path::new(&s)
            .parent()
            .map(|d| d.to_path_buf())
    }

    /// Where SpeedForce.ini lives.
    ///
    /// Order: `SPEEDFORCE_INI` env (tests / multiple profiles) -> next to the
    /// DLL -> next to the game exe (the pre-L location, still honoured so an
    /// existing install keeps working). The second return value is a note for
    /// the log whenever the answer was not the preferred one.
    fn ini_path() -> (std::path::PathBuf, Option<String>) {
        if let Some(p) = env("SPEEDFORCE_INI") {
            return (std::path::PathBuf::from(p), None);
        }
        let name = config_file::DEFAULT_INI_FILE;
        let preferred = dll_dir().unwrap_or_else(exe_dir).join(name);
        if preferred.is_file() {
            return (preferred, None);
        }
        let legacy = exe_dir().join(name);
        if legacy.is_file() && legacy != preferred {
            // Build the message before moving `legacy`, otherwise the format
            // borrows an already-moved value.
            let note = format!(
                "read the INI from the legacy location {}; move it to {}",
                legacy.display(),
                preferred.display()
            );
            return (legacy, Some(note));
        }
        (
            preferred,
            Some(format!("no {} next to the DLL or the game exe", name)),
        )
    }

    /// Default log file when `SPEEDFORCE_M4E_LOG` is not set.
    ///
    /// Without this the DLL runs silently, which makes every real-world problem
    /// undiagnosable. `natives\logs\speedforce.jsonl` is inside the mod folder,
    /// so it cannot be lost in the game directory.
    fn default_log_path() -> Option<std::path::PathBuf> {
        let dir = dll_dir().or_else(|| Some(exe_dir()))?;
        let logs = dir.join("logs");
        std::fs::create_dir_all(&logs).ok()?;
        Some(logs.join("speedforce.jsonl"))
    }

    /// Load + validate the INI, then let env override it.
    fn load_settings() -> config_file::Settings {
        let (path, note) = ini_path();
        let path_str = path.to_string_lossy().to_string();
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let mut s = config_file::Settings::from_ini_text(&text, &path_str, true);
                s.apply_env_overrides(env);
                if let Some(n) = note {
                    s.warnings.push(n);
                }
                s
            }
            Err(e) => {
                let mut s = config_file::Settings::from_ini_text("", &path_str, false);
                if let Some(n) = note {
                    s.warnings.push(n);
                }
                s.warnings.push(format!(
                    "cannot read {}: {} -> using defaults",
                    path_str,
                    e.kind()
                ));
                s.apply_env_overrides(env);
                s
            }
        }
    }

    /// First error decides the halt reason, so the log says *which* value was
    /// refused instead of a generic "bad config".
    fn halt_reason(s: &config_file::Settings) -> Option<&'static str> {
        if s.errors.is_empty() {
            return None;
        }
        let first = &s.errors[0];
        Some(if first.contains("Radius") {
            "CONFIG_INVALID_RADIUS"
        } else if first.contains("SlowScale") {
            "CONFIG_INVALID_SCALE"
        } else if first.contains("DurationMs") {
            "CONFIG_INVALID_DURATION"
        } else if first.contains("MaxTargets") {
            "CONFIG_INVALID_MAX_TARGETS"
        } else {
            "CONFIG_INVALID"
        })
    }

    /// INI/env-derived core config. Only gameplay values come from the file;
    /// offsets, the identity chain and the write backend stay hard-coded.
    fn config(s: &config_file::Settings) -> Config {
        Config {
            armed: s.enabled,
            player_chr_ptr: env("SPEEDFORCE_M4E_CHR_PTR")
                .as_deref()
                .and_then(hex)
                .unwrap_or(0),
            speed_offset: env("SPEEDFORCE_M4E_SPEED_OFFSET")
                .as_deref()
                .and_then(hex)
                .unwrap_or(DEFAULT_SPEED_OFFSET),
            scale: s.slow_scale,
            duration_ms: s.duration_ms,
            max_targets: s.max_targets,
            max_list_nodes: env("SPEEDFORCE_M4F_MAX_LIST_NODES")
                .map(|v| dec_usize(&v, DEFAULT_MAX_LIST_NODES))
                .unwrap_or(DEFAULT_MAX_LIST_NODES),
            wcm_rva: DEFAULT_WCM_RVA,
            max_candidates: env("SPEEDFORCE_M4F_MAX_CANDIDATES")
                .map(|v| dec_usize(&v, MAX_CANDIDATE_SCAN))
                .unwrap_or(MAX_CANDIDATE_SCAN),
            player_off: DEFAULT_PLAYER_OFF,
            target_limit: s.target_limit,
            radius_m: s.radius_m,
            page_check: env("SPEEDFORCE_PAGE_CHECK").as_deref() == Some("1"),
            hold_probe_targets: env("SPEEDFORCE_M4F_HOLD_PROBES")
                .map(|v| dec_usize(&v, 2))
                .unwrap_or(2),
            hotkey_enabled: s.hotkey_enabled,
            debug_log: s.debug_log,
            forced_halt: halt_reason(s),
        }
    }

    fn json_string(s: &str) -> String {
        let mut out = String::from("\"");
        for ch in s.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    /// One syscall per tick: all pending events are batched into a single write.
    fn flush_events(c: &mut Controller<WinBackend>, fh: &mut Option<std::fs::File>) {
        use std::io::Write;
        let events = c.take_events();
        if events.is_empty() {
            return;
        }
        if let Some(f) = fh.as_mut() {
            let mut out = String::with_capacity(events.len() * 120);
            for e in events {
                out.push_str(&format!(
                    "{{\"t_ms\":{},\"kind\":\"{}\",\"detail\":{}}}\n",
                    e.t_ms,
                    e.kind,
                    json_string(&e.detail)
                ));
            }
            let _ = f.write_all(out.as_bytes());
        }
    }

    /// Write the whole configuration decision into the event log: where the
    /// file was, what each key resolved to and from which layer, what was
    /// rejected, and which sections are parsed-but-reserved.
    ///
    /// This is the part that makes "Enabled=1 but nothing happened" impossible:
    /// either the value is in force and logged, or it is reported as
    /// NOT_IMPLEMENTED / CONFIG_ERROR.
    fn log_settings(c: &mut Controller<WinBackend>, s: &config_file::Settings) {
        c.emit(
            "CONFIG_FILE",
            format!("path={} found={}", s.ini_path, s.ini_found),
        );

        // Only the LAST occurrence of each key matters; earlier ones were
        // overridden (ini -> env) and would just add noise.
        let mut keys: Vec<&str> = Vec::new();
        for (k, _, _) in s.resolved.iter().rev() {
            if !keys.contains(&k.as_str()) {
                keys.push(k.as_str());
            }
        }
        keys.reverse();
        for k in keys {
            if let Some((_, v, src)) = s.resolved.iter().rev().find(|(kk, _, _)| kk == k) {
                c.emit("CONFIG", format!("{}={} source={}", k, v, src));
            }
        }

        for w in &s.warnings {
            c.emit("CONFIG_WARN", w.clone());
        }
        for r in &s.reserved {
            c.emit(
                "NOT_IMPLEMENTED",
                format!("section={} enabled={} {}", r.section, r.enabled as u8, r.detail),
            );
        }
        for e in &s.errors {
            c.emit("CONFIG_ERROR", e.clone());
        }
        if !s.errors.is_empty() {
            c.emit(
                "CONFIG_INVALID",
                format!(
                    "{} rejected value(s) -> halting rather than running with a value that                      was not requested",
                    s.errors.len()
                ),
            );
        }
    }

    extern "system" fn worker(_param: *mut c_void) -> u32 {
        // Log path: env wins, otherwise a default next to the DLL. Running with
        // no log at all is how a silent failure becomes an unfixable one.
        let (log_path, log_source) = match env("SPEEDFORCE_M4E_LOG") {
            Some(p) => (Some(p), "env"),
            None => (
                default_log_path().map(|p| p.to_string_lossy().to_string()),
                "default",
            ),
        };
        let mut fh = log_path.as_ref().and_then(|p| {
            std::fs::OpenOptions::new().create(true).append(true).open(p).ok()
        });
        let settings = load_settings();
        let cfg = config(&settings);
        let static_player_ptr = cfg.player_chr_ptr;
        let backend = WinBackend {
            static_player_ptr,
            target_file: env("SPEEDFORCE_M4E_TARGET_FILE"),
            slow_write: env("SPEEDFORCE_SLOW_WRITE").as_deref() == Some("1"),
        };
        let mut c = Controller::new(cfg, backend);
        EXIT_STATUS.store(1, Ordering::SeqCst);
        c.log_mode();
        c.emit(
            "LOG_FILE",
            format!(
                "path={} source={} opened={}",
                log_path.as_deref().unwrap_or("<none>"),
                log_source,
                fh.is_some() as u8
            ),
        );
        log_settings(&mut c, &settings);

        // the 神奇素 detector lives here, outside the slowdown core, so
        // the verified activation/restore path is untouched.
        let mut det = sp_signal::Detector::new(
            settings.item_trigger_live,
            settings.item_signal_speffect_id,
            settings.poll_interval_ms,
        );
        // Sleeping shorter than the core tick only when the detector wants it;
        // the poll interval is a floor, the real period is whatever the loop
        // achieves and it is measured, not assumed.
        let sleep_ms = if det.enabled {
            TICK_MS.min(det.poll_ms.max(1))
        } else {
            TICK_MS
        };
        c.emit(
            "SIGNAL_DETECTOR",
            format!(
                "enabled={} signal_id={} poll_ms={} loop_sleep_ms={} effective_ms={} dry_run={}",
                det.enabled as u8, det.signal_id, det.poll_ms, sleep_ms,
                det.poll_ms.max(sleep_ms), settings.item_dry_run as u8
            ),
        );
        let mut last_stats_ms = c.backend.now_ms();
        let mut last_stats_cpu = c.backend.thread_cpu_ns();
        // host-side count of rising edges, independent of the detector's
        // internal meter. Every edge must be accountable in the log.
        let mut edge_seq: u64 = 0;
        flush_events(&mut c, &mut fh);

        while !STOP.load(Ordering::SeqCst) {
            c.tick();

            let now = c.backend.now_ms();
            match det.poll(&mut c.backend, &c.cfg, now) {
                sp_signal::Poll::RisingEdge => {
                    edge_seq += 1;
                    let state_before = c.state_label();
                    c.emit(
                        "ITEM_SIGNAL_RISING_EDGE",
                        format!("signal_id={} source=Item edge_no={}", det.signal_id, edge_seq),
                    );
                    // P1 phase 1 evidence: emitted on EVERY edge, dry-run
                    // included. It names the exact function the edge is routed
                    // to, so "is the wiring in place?" is answerable from the
                    // log without running the slowdown.
                    c.emit(
                        "ITEM_TRIGGER_DISPATCH",
                        format!(
                            "edge_no={} source=Item core_entry=Controller::request_activation \
                             state_before={} dry_run={} will_call_core={} \
                             reject_while_active=1 require_rising_edge=1",
                            edge_seq, state_before, settings.item_dry_run as u8,
                            (!settings.item_dry_run) as u8
                        ),
                    );

                    if settings.item_dry_run {
                        c.emit(
                            "ITEM_TRIGGER_DRYRUN",
                            format!(
                                "signal_id={} edge_no={} \
                                 note=edge_validated_core_would_be_called \
                                 path=request_activation(Item) no_write=1",
                                det.signal_id, edge_seq
                            ),
                        );
                    } else {
                        // Phase 2+: the actual call into the H core. Emitted
                        // BEFORE the call so a crash/failure still leaves proof
                        // that the entry point was reached.
                        c.emit(
                            "ITEM_CORE_INVOKED",
                            format!(
                                "edge_no={} entry=Controller::request_activation \
                                 source=Item state_before={} \
                                 chain=select_targets->verify->write->15s->restore_all \
                                 same_entry_as=DebugHotkey",
                                edge_seq, state_before
                            ),
                        );
                        let started = c.request_activation(TriggerSource::Item, now);
                        let refusal = c.last_refusal().unwrap_or("-").to_string();
                        let rep = c.last_enum.clone().unwrap_or_default();
                        c.emit(
                            if started {
                                "ITEM_TRIGGER_STARTED"
                            } else {
                                "ITEM_TRIGGER_REJECTED"
                            },
                            format!(
                                "signal_id={} edge_no={} started={} reason={} \
                                 state_after={} targets={} written={} \
                                 duration_ms={} restore=AUTO_TIMEOUT",
                                det.signal_id, edge_seq, started as u8, refusal,
                                c.state_label(), c.targets.len(), rep.written,
                                c.cfg.duration_ms
                            ),
                        );
                        if !started {
                            // One line with the actual cause. Previously the
                            // log only said `started=0` and the operator had to
                            // correlate ACTIVATION_REFUSED / TRIGGER_REJECTED.
                            c.emit(
                                "ITEM_TRIGGER_FAILURE",
                                format!(
                                    "edge_no={} reason={} \
                                     note=no_slowdown_started_see_ACTIVATION_REFUSED_or_TRIGGER_REJECTED",
                                    edge_seq, refusal
                                ),
                            );
                        }
                        if state_before == "Active" {
                            // The game consumes the item in TAE UseGoods, BEFORE
                            // this SpEffect ever appears. Refusing a repeat here
                            // cannot give the item back. Explicitly logged, and
                            // explicitly NOT claimed to be fixed.
                            c.emit(
                                "ITEM_CONSUMED_WHILE_ACTIVE",
                                format!(
                                    "edge_no={} reason=ALREADY_ACTIVE \
                                     note=item_already_consumed_by_game_no_refund \
                                     issue=OPEN",
                                    edge_seq
                                ),
                            );
                        }
                    }
                }
                sp_signal::Poll::Baseline { present } => {
                    c.emit(
                        "ITEM_SIGNAL_BASELINE",
                        format!(
                            "present={} note=startup_or_player_change_no_trigger",
                            present as u8
                        ),
                    );
                }
                sp_signal::Poll::Unavailable(reason) => {
                    if det.take_new_reason(reason) {
                        c.emit("SP_SIGNAL_UNAVAILABLE", format!("reason={}", reason));
                    }
                }
                _ => {}
            }

            // Periodic meter so "10 ms polling is cheap" is a measurement, not a
            // claim. 30 s cadence: 3 lines per minute at most.
            let now2 = c.backend.now_ms();
            if now2.saturating_sub(last_stats_ms) >= 30_000 {
                let cpu_now = c.backend.thread_cpu_ns();
                let s = &det.stats;
                c.emit(
                    "SIGNAL_STATS",
                    format!(
                        "polls={} sampled={} unavailable={} edges={} baselines={}                          player_changes={} reads={} read_failures={} chain_max={}                          last_interval_ms={} max_interval_ms={}                          window_ms={} worker_cpu_ms={}",
                        s.polls, s.sampled, s.unavailable, s.edges, s.baselines,
                        s.player_changes, s.reads, s.read_failures, s.chain_max,
                        s.last_interval_ms, s.max_interval_ms,
                        now2.saturating_sub(last_stats_ms),
                        cpu_now.saturating_sub(last_stats_cpu) / 1_000_000
                    ),
                );
                last_stats_ms = now2;
                last_stats_cpu = cpu_now;
            }

            flush_events(&mut c, &mut fh);
            unsafe { Sleep(sleep_ms as u32) };
        }
        let status = match c.shutdown_restore() {
            RestoreOutcome::NotActive => 2,
            RestoreOutcome::Restored => 3,
            RestoreOutcome::RestoreFailed => 4,
            RestoreOutcome::WriteForbidden => 5,
        };
        flush_events(&mut c, &mut fh);
        EXIT_STATUS.store(status, Ordering::SeqCst);
        0
    }

    #[no_mangle]
    pub extern "system" fn DllMain(
        module: *mut c_void,
        reason: u32,
        _reserved: *mut c_void,
    ) -> i32 {
        unsafe {
            match reason {
                DLL_PROCESS_ATTACH => {
                    DLL_HINST.store(module as isize, Ordering::SeqCst);
                    DisableThreadLibraryCalls(module);
                    EXIT_STATUS.store(0, Ordering::SeqCst);
                    let mut tid: u32 = 0;
                    let h = CreateThread(ptr::null_mut(), 0, worker, ptr::null_mut(), 0, &mut tid);
                    WORKER_HANDLE.store(h as isize, Ordering::SeqCst);
                }
                DLL_PROCESS_DETACH => {
                    // NOT a guaranteed restore: loader lock held, worker may be
                    // suspended. Only the explicit finalizer joins the worker.
                    STOP.store(true, Ordering::SeqCst);
                    if !FINALIZER_CALLED.load(Ordering::SeqCst) {
                        EXIT_STATUS.store(7, Ordering::SeqCst);
                    }
                }
                _ => {}
            }
        }
        1
    }

    /// Verified shutdown: joins the worker, then reports its terminal status.
    /// 2 nothing to restore / 3 restored / 4 restore failed / 5 write forbidden
    /// / 6 timeout (NOT guaranteed). 4/5/6/7 => restart the game.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4G_Finalize() -> i32 {
        FINALIZER_CALLED.store(true, Ordering::SeqCst);
        STOP.store(true, Ordering::SeqCst);
        let h = WORKER_HANDLE.load(Ordering::SeqCst) as *mut c_void;
        if h.is_null() {
            return EXIT_STATUS.load(Ordering::SeqCst);
        }
        let rc = unsafe { WaitForSingleObject(h, FINALIZER_WAIT_MS) };
        if rc != 0 {
            EXIT_STATUS.store(6, Ordering::SeqCst);
            return 6;
        }
        unsafe { CloseHandle(h) };
        WORKER_HANDLE.store(0, Ordering::SeqCst);
        EXIT_STATUS.load(Ordering::SeqCst)
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4G_ExitStatus() -> i32 {
        EXIT_STATUS.load(Ordering::SeqCst)
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4G_Version() -> u32 {
        10
    }

    /// H aliases. The G .me3 profiles keep working unchanged (they call
    /// the M4G names), while the H scripts can use the milestone-correct
    /// names. Both resolve to the same code path.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4H_Finalize() -> i32 {
        SpeedForce_M4G_Finalize()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4H_ExitStatus() -> i32 {
        SpeedForce_M4G_ExitStatus()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4H_Version() -> u32 {
        SpeedForce_M4G_Version()
    }

    /// J aliases: same entry points under the milestone-correct names, so a
    /// profile can ask for M4J_Finalize while the older names keep working.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4J_Finalize() -> i32 {
        SpeedForce_M4G_Finalize()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4J_ExitStatus() -> i32 {
        SpeedForce_M4G_ExitStatus()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4J_Version() -> u32 {
        SpeedForce_M4G_Version()
    }

    /// L aliases. The shipped .me3 asks for M4J_Finalize, so that name must
    /// keep working; these exist for milestone-correct references.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4L_Finalize() -> i32 {
        SpeedForce_M4G_Finalize()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4L_ExitStatus() -> i32 {
        SpeedForce_M4G_ExitStatus()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4L_Version() -> u32 {
        SpeedForce_M4G_Version()
    }

    /// P aliases. The shipped .me3 asks for `SpeedForce_M4J_Finalize`, so
    /// every older name must keep resolving; these are for milestone-correct
    /// references.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P_Finalize() -> i32 {
        SpeedForce_M4G_Finalize()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P_ExitStatus() -> i32 {
        SpeedForce_M4G_ExitStatus()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P_Version() -> u32 {
        SpeedForce_M4G_Version()
    }

    /// P1 aliases. `EldenRingMods.me3` asks for M4J_Finalize, which stays
    /// exported; these names exist for milestone-correct references.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P1_Finalize() -> i32 {
        SpeedForce_M4G_Finalize()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P1_ExitStatus() -> i32 {
        SpeedForce_M4G_ExitStatus()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P1_Version() -> u32 {
        SpeedForce_M4G_Version()
    }

    /// P3 aliases. `EldenRingMods.me3` asks for M4J_Finalize, which stays
    /// exported; these names exist for milestone-correct references.
    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P3_Finalize() -> i32 {
        SpeedForce_M4G_Finalize()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P3_ExitStatus() -> i32 {
        SpeedForce_M4G_ExitStatus()
    }

    #[no_mangle]
    pub extern "C" fn SpeedForce_M4P3_Version() -> u32 {
        SpeedForce_M4G_Version()
    }
}

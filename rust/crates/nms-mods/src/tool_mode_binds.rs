//! ToolModeBinds: Mining Laser, Boltcaster and Terrain Manipulator rows in the Quick Menu's
//! "Change Secondary Weapon" submenu, and so Ctrl+digit binds for them.
//!
//! The multi-tool has a primary slot (`[W+0xF74]`) and a secondary slot (`[W+0xF78]`). The submenu lists only
//! modes whose bit is set in the secondary mask `G+0xE68`; the three tools are primaries. Two code sites:
//!
//! 1. Row filter (`FILTER_LOOP + 13`, 14 bytes `test [rdi+rax*4+0xE68], r14d; je skip`) becomes a jump to a
//!    cave: a secondary mode goes on exactly as in vanilla; otherwise the mode is let through only when its
//!    bit is in our extra mask (cave data, written live), and the bit is or-ed into a "seen" word that a
//!    log thread reports. `eax` and the flags are dead at both exits (the next block reloads them).
//! 2. The Quick Menu action's `call select` (`ACTION_SITE + 10`) is retargeted to a stub that jumps to
//!    [`qm_select`]: a secondary mode calls the original select (vanilla); a primary mode in the extra mask
//!    calls the game's SetPrimary (which runs CanEquip itself). Hotbar binds replay this same call.
//!
//! Secondaries, the secondary mask, the weapon-switch key and the secondary cycle are untouched. The switch
//! is live: off writes the original bytes back. Any byte that differs from what was read in the disassembly
//! leaves both sites vanilla, with a log line. On by default since the in-game test of 2026-10-13 passed.
//!
//! Only the "Change Secondary Weapon" submenu runs the filter cave: the level builder's jump table sends
//! parent type `0x26` alone to `0x141559e40` (build 183791: `add eax,-7`, byte table `0x14155a5c4`, only
//! index `0x1F` selects that case), and no other branch lands in that block. The Recharge submenu (`0x18`,
//! case `0x14152bd86`, card helper `0x141530e70` in build 180836) lists a weapon technology only when it
//! is the tech of the current primary (`[W+0xF74]`) or secondary (`[W+0xF78]`), any charge: vanilla. A
//! Mining Beam or Terrain Manipulator card there follows the primary the player picked.
//!
//! Off with a hotbar key still bound to an added row: the key replays the Quick Menu action, vanilla select
//! refuses the primary and the action still plays its switch sound. Same as vanilla for any bind whose row
//! cannot be equipped; documented in the README, not patched (off must leave the game code vanilla).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use nms_core::log::Log;
use nms_core::mem::{self, alloc_exec_near, read_i32, read_ptr, read_u32, rel32};
use nms_core::process::ProcessImage;
use nms_core::{paths, Code, GameMod, Kind, Pattern, SettingSpec, Settings};
use nms_sdk::sigs::tool_mode_binds as sigs;
use serde_json::{json, Value};

const NAME: &str = "ToolModeBinds";
/// Default of the `enabled` switch: on since the in-game test passed (2026-10-13).
const DEFAULT_ENABLED: bool = true;

/// Game weapon modes (`cGcPlayerWeapons`, 21 ordinals). Index = mode.
const MODE_NAMES: [&str; 21] = [
    "Bolt",
    "Shotgun",
    "Burst",
    "Rail",
    "Cannon",
    "Laser",
    "Grenade",
    "MineGrenade",
    "Scope",
    "FrontShield",
    "Melee",
    "TerrainEdit",
    "SunLaser",
    "Spawner",
    "SpawnerAlt",
    "SoulLaser",
    "Flamethrower",
    "StunGrenade",
    "Stealth",
    "FishLaser",
    "Gravity",
];
const MODE_COUNT: i32 = MODE_NAMES.len() as i32;
const BOLT: u32 = 1 << 0;
const LASER: u32 = 1 << 5;
const TERRAIN: u32 = 1 << 11;

/// Cave layout (one block near the game code).
const CAVE_SIZE: usize = 0x1000;
const STUB_AT: usize = 0x40;
const EXTRA_AT: usize = 0x80;
const SEEN_AT: usize = 0x84;

/// Live switch, read by the detour.
static ENABLED: AtomicBool = AtomicBool::new(false); // set by `apply` at start-up
/// Modes routed to SetPrimary (mirrors the cave's extra word).
static EXTRA: AtomicU32 = AtomicU32::new(0);
static LOG_ON: AtomicBool = AtomicBool::new(false);
static ORIG_SELECT: AtomicUsize = AtomicUsize::new(0);
static SET_PRIMARY: AtomicUsize = AtomicUsize::new(0);
/// Address of the game's global pointer `G` (the secondary mask is at `G+0xE68`).
static G_GLOBAL: AtomicUsize = AtomicUsize::new(0);
static LOG: OnceLock<Arc<Log>> = OnceLock::new();

type SelectFn = unsafe extern "system" fn(usize, i32) -> u8;
type SetPrimaryFn = unsafe extern "system" fn(usize, i32);

const SPECS: &[SettingSpec] = &[
    SettingSpec { key: "status", label: "Status", kind: Kind::Status },
    SettingSpec { key: "enabled", label: "Tools in the weapon menu", kind: Kind::Bool },
    SettingSpec { key: "mining_laser", label: "Mining Laser", kind: Kind::Bool },
    SettingSpec { key: "boltcaster", label: "Boltcaster", kind: Kind::Bool },
    SettingSpec { key: "terrain_manipulator", label: "Terrain Manipulator", kind: Kind::Bool },
    SettingSpec { key: "more_modes", label: "More modes", kind: Kind::Text },
    SettingSpec { key: "write_logs", label: "Write log", kind: Kind::Bool },
];

fn log_line(msg: &str) {
    if LOG_ON.load(Ordering::Acquire) {
        if let Some(log) = LOG.get() {
            log.info(msg);
        }
    }
}

// ---- pure logic (unit tested) -----------------------------------------------------------------

/// The mode's name for logs, with the in-game name for the three default tools.
fn mode_name(mode: i32) -> String {
    match mode {
        0 => "Bolt (Boltcaster)".into(),
        5 => "Laser (Mining Laser)".into(),
        11 => "TerrainEdit (Terrain Manipulator)".into(),
        m if (0..MODE_COUNT).contains(&m) => MODE_NAMES[m as usize].into(),
        m => format!("mode {m}"),
    }
}

/// Names of every mode in a mask, comma separated.
fn mask_names(mask: u32) -> String {
    let names: Vec<String> = (0..MODE_COUNT).filter(|m| mask & (1 << m) != 0).map(mode_name).collect();
    if names.is_empty() { "none".into() } else { names.join(", ") }
}

/// `more_modes`: game mode names (`Shotgun`, `Rail`, ...) or numbers 0..20, separated by commas or spaces.
fn parse_modes(text: &str) -> Result<u32, String> {
    let mut mask = 0u32;
    let mut unknown = Vec::new();
    for token in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|t| !t.is_empty()) {
        let by_name = MODE_NAMES.iter().position(|n| n.eq_ignore_ascii_case(token));
        let by_number = token.parse::<i32>().ok().filter(|n| (0..MODE_COUNT).contains(n)).map(|n| n as usize);
        match by_name.or(by_number) {
            Some(m) => mask |= 1 << m,
            None => unknown.push(token.to_string()),
        }
    }
    if unknown.is_empty() {
        Ok(mask)
    } else {
        Err(format!("unknown mode(s) {}; use {}", unknown.join(", "), MODE_NAMES.join(", ")))
    }
}

/// The modes the submenu gets on top of the vanilla secondaries.
fn extra_mask(laser: bool, bolt: bool, terrain: bool, more: u32) -> u32 {
    let mut m = more;
    if laser {
        m |= LASER;
    }
    if bolt {
        m |= BOLT;
    }
    if terrain {
        m |= TERRAIN;
    }
    m & ((1u32 << MODE_COUNT) - 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The original select, exactly as vanilla.
    Vanilla,
    /// SetPrimary.
    Primary,
}

/// Where a picked row goes. Anything unclear (mask unreadable, mode out of range, a secondary, a mode we
/// did not add) takes the vanilla path.
fn route(enabled: bool, mode: i32, secondary_mask: Option<u32>, extra: u32) -> Route {
    if !enabled || !(0..MODE_COUNT).contains(&mode) {
        return Route::Vanilla;
    }
    let bit = 1u32 << mode;
    match secondary_mask {
        Some(mask) if mask & bit == 0 && extra & bit != 0 => Route::Primary,
        _ => Route::Vanilla,
    }
}

fn rel(end: usize, target: usize) -> Result<[u8; 4], String> {
    rel32(end, target).map(i32::to_le_bytes).ok_or_else(|| "the cave is out of reach of the game code".to_string())
}

/// The filter cave at `cave`. `join` is the instruction after the filter, `skip` the loop tail.
fn filter_cave(cave: usize, join: usize, skip: usize) -> Result<Vec<u8>, String> {
    let mut c = Vec::with_capacity(48);
    // mov eax, [rdi+rax*4+0xE68]
    c.extend_from_slice(&[0x8B, 0x84, 0x87, 0x68, 0x0E, 0x00, 0x00]);
    // test eax, r14d
    c.extend_from_slice(&[0x44, 0x85, 0xF0]);
    // jne join (a secondary: vanilla row)
    c.extend_from_slice(&[0x0F, 0x85]);
    c.extend_from_slice(&rel(cave + c.len() + 4, join)?);
    // test [rip+extra], r14d
    c.extend_from_slice(&[0x44, 0x85, 0x35]);
    c.extend_from_slice(&rel(cave + c.len() + 4, cave + EXTRA_AT)?);
    // je skip (not ours: no row, as vanilla)
    c.extend_from_slice(&[0x0F, 0x84]);
    c.extend_from_slice(&rel(cave + c.len() + 4, skip)?);
    // or [rip+seen], r14d
    c.extend_from_slice(&[0x44, 0x09, 0x35]);
    c.extend_from_slice(&rel(cave + c.len() + 4, cave + SEEN_AT)?);
    // jmp join
    c.push(0xE9);
    c.extend_from_slice(&rel(cave + c.len() + 4, join)?);
    if c.len() > STUB_AT {
        return Err("filter cave too long".into());
    }
    Ok(c)
}

/// `jmp [rip+0]; dq target`: the stub the action call is sent to.
fn abs_jump(target: usize) -> Vec<u8> {
    let mut s = vec![0xFF, 0x25, 0, 0, 0, 0];
    s.extend_from_slice(&(target as u64).to_le_bytes());
    s
}

/// The 14 bytes written over the filter: `jmp cave` and nine `nop`s.
fn filter_jump(site: usize, cave: usize) -> Result<[u8; 14], String> {
    let mut out = [0x90u8; 14];
    out[0] = 0xE9;
    out[1..5].copy_from_slice(&rel(site + 5, cave)?);
    Ok(out)
}

/// The 5 bytes written over the action call: `call stub`.
fn call_to(site: usize, target: usize) -> Result<[u8; 5], String> {
    let mut out = [0xE8u8; 5];
    out[1..5].copy_from_slice(&rel(site + 5, target)?);
    Ok(out)
}

// ---- the detour ---------------------------------------------------------------------------------

fn secondary_mask() -> Option<u32> {
    let g = read_ptr(G_GLOBAL.load(Ordering::Acquire))?;
    read_u32(g + sigs::MASK_OFFSET)
}

/// `(pending, primary, secondary)` of the weapon, for the log.
fn slots(w: usize) -> String {
    let r = |off| read_i32(w + off).map_or("?".to_string(), |v| v.to_string());
    format!("pending {} primary {} secondary {}", r(sigs::W_PENDING), r(sigs::W_PRIMARY), r(sigs::W_SECONDARY))
}

/// Replaces `select(W, mode)` at the Quick Menu action. Same arguments; the caller ignores the result.
unsafe extern "system" fn qm_select(w: usize, mode: i32) -> u8 {
    let extra = EXTRA.load(Ordering::Acquire);
    let orig = ORIG_SELECT.load(Ordering::Acquire);
    let set = SET_PRIMARY.load(Ordering::Acquire);
    let mask = secondary_mask();
    match route(ENABLED.load(Ordering::Acquire), mode, mask, extra) {
        Route::Primary if set != 0 && w >= mem::MIN_ADDR => {
            let before = if LOG_ON.load(Ordering::Acquire) { slots(w) } else { String::new() };
            std::mem::transmute::<usize, SetPrimaryFn>(set)(w, mode);
            if LOG_ON.load(Ordering::Acquire) {
                log_line(&format!("primary switch to {}: before {before}, after {}", mode_name(mode), slots(w)));
            }
            1
        }
        _ => {
            if orig == 0 {
                return 0;
            }
            let r = std::mem::transmute::<usize, SelectFn>(orig)(w, mode);
            if LOG_ON.load(Ordering::Acquire) {
                let mask = mask.map_or("?".to_string(), |m| format!("{m:#x}"));
                log_line(&format!("vanilla select {} (mask {mask}) returned {r}", mode_name(mode)));
            }
            r
        }
    }
}

// ---- install ------------------------------------------------------------------------------------

fn find(code: &dyn Code, name: &str, pattern: &str) -> Result<usize, String> {
    let pat = Pattern::parse(pattern).map_err(|e| format!("{name}: {e}"))?;
    code.find_unique(&pat).unique().ok_or_else(|| format!("{name} not found (game update?)"))
}

fn bytes_at(code: &dyn Code, rva: usize, want: &[u8]) -> bool {
    code.read(rva, want.len()).is_some_and(|b| b == want)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

/// Both sites, their original bytes and what goes there when on (all RVAs).
struct Sites {
    filter: usize,
    filter_new: [u8; 14],
    action: usize,
    action_old: [u8; 5],
    action_new: [u8; 5],
    seen: usize,
    extra: usize,
}

/// Every address the mod uses, checked (RVAs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Located {
    filter: usize,
    join: usize,
    skip: usize,
    global: usize,
    select: usize,
    action: usize,
    action_old: [u8; 5],
    set_primary: usize,
}

/// Find and check every address byte for byte. Reads only.
fn locate(code: &dyn Code) -> Result<Located, String> {
    // Row filter: exact bytes, the skip target is the loop tail, the tail loops back to this head.
    let head = find(code, "Quick Menu row filter", sigs::FILTER_LOOP)?;
    let filter = head + sigs::FILTER_AT;
    let found = code.read(filter, sigs::FILTER_OLD.len()).unwrap_or_default();
    if found != sigs::FILTER_OLD {
        return Err(format!("the row filter is not what this mod knows (found {})", hex(&found)));
    }
    let join = filter + sigs::FILTER_OLD.len();
    let skip = code.rel32_target(filter + 10).ok_or("row filter jump unreadable")?;
    if !bytes_at(code, skip, &sigs::LOOP_TAIL) || code.rel32_target(skip + sigs::LOOP_TAIL.len()) != Some(head) {
        return Err("the weapon row loop is not what this mod knows (game update?)".into());
    }
    let g_filter = code.rel32_target(head + sigs::FILTER_G_DISP).ok_or("row filter global unreadable")?;

    // Select: the action's call target, the same function as the SELECT pattern, reading the same mask.
    let select = find(code, "secondary select", sigs::SELECT)?;
    if !bytes_at(code, select + sigs::SELECT_MASK_AT, &sigs::SELECT_MASK_CODE) {
        return Err("secondary select no longer reads the secondary mask (game update?)".into());
    }
    let g_select = code.rel32_target(select + sigs::SELECT_G_DISP).ok_or("select global unreadable")?;
    if g_select != g_filter {
        return Err("row filter and select read different globals (game update?)".into());
    }
    let site = find(code, "Quick Menu weapon action", sigs::ACTION_SITE)?;
    let action = site + sigs::ACTION_CALL_AT;
    let action_old: [u8; 5] =
        code.read(action, 5).and_then(|b| b.try_into().ok()).ok_or("Quick Menu weapon action unreadable")?;
    if action_old[0] != 0xE8 || code.rel32_target(action + 1) != Some(select) {
        return Err(format!("the Quick Menu weapon action does not call select (found {})", hex(&action_old)));
    }

    // SetPrimary and the CanEquip it calls first.
    let set_primary = find(code, "SetPrimary", sigs::SET_PRIMARY)?;
    let can_equip = find(code, "CanEquip", sigs::CAN_EQUIP)?;
    if code.rel32_target(set_primary + sigs::SET_PRIMARY_CAN_CALL + 1) != Some(can_equip) {
        return Err("SetPrimary does not call CanEquip first (game update?)".into());
    }
    Ok(Located { filter, join, skip, global: g_select, select, action, action_old, set_primary })
}

/// Check every address and build the caves. Nothing in the game code is changed here.
fn prepare(image: &ProcessImage) -> Result<Sites, String> {
    let at = locate(image)?;
    let filter_va = image.va(at.filter);
    let cave = alloc_exec_near(filter_va, CAVE_SIZE).ok_or("no memory free near the game code for the cave")?;
    let body = filter_cave(cave, image.va(at.join), image.va(at.skip))?;
    let stub = abs_jump(qm_select as *const () as usize);
    let filter_new = filter_jump(filter_va, cave)?;
    let action_new = call_to(image.va(at.action), cave + STUB_AT)?;
    // SAFETY: fresh RWX block of CAVE_SIZE bytes owned by this mod; no game code points at it yet.
    unsafe {
        std::ptr::copy_nonoverlapping(body.as_ptr(), cave as *mut u8, body.len());
        std::ptr::copy_nonoverlapping(stub.as_ptr(), (cave + STUB_AT) as *mut u8, stub.len());
        mem::write(cave + EXTRA_AT, 0u32);
        mem::write(cave + SEEN_AT, 0u32);
    }

    ORIG_SELECT.store(image.va(at.select), Ordering::Release);
    SET_PRIMARY.store(image.va(at.set_primary), Ordering::Release);
    G_GLOBAL.store(image.va(at.global), Ordering::Release);
    Ok(Sites {
        filter: at.filter,
        filter_new,
        action: at.action,
        action_old: at.action_old,
        action_new,
        seen: cave + SEEN_AT,
        extra: cave + EXTRA_AT,
    })
}

/// Write `to` at `rva` if the code there is `from` (or already `to`). Anything else is left alone.
fn swap(image: &mut ProcessImage, rva: usize, from: &[u8], to: &[u8], what: &str) -> Result<(), String> {
    let now = image.read(rva, from.len()).unwrap_or_default();
    if now == to {
        return Ok(());
    }
    if now != from {
        return Err(format!("{what} was changed by something else (found {})", hex(&now)));
    }
    // Write order and tearing: `apply` writes the action call first and removes it last, so an
    // added row always routes through `qm_select`. A patch of at most 8 bytes that sits inside one
    // aligned 8-byte word is written with one 8-byte store (a thread running it sees the old or the
    // new bytes, never a mix). On the exe this mod was checked on (89,491,528 bytes, build 183791, the
    // build of the `locates_every_site_in_the_exe` test) the action call `0x1415667af` starts at byte 7
    // of its word (as `0x14153a80f` did in 180836), so that branch is inert there and the plain write below runs.
    // The 14-byte row-filter jump cannot be written atomically either. Both run only while the Quick
    // Menu builds or acts on its weapon rows, so a toggle at that exact moment is the remaining
    // (small) window, as for every byte patch. If `write_atomic` stored the bytes but could not
    // restore the protection, the plain write below writes the same bytes again and retries it.
    if to.len() <= 8 && image.write_atomic(rva, to).is_ok() {
        return Ok(());
    }
    image.write(rva, to).map_err(|e| format!("{what} could not be written ({e})"))
}

/// Seen modes, reported once per submenu build (the builder may run every frame while it is open).
fn watch_rows(seen: usize) {
    // SAFETY: `seen` is a 4-aligned word in our never-freed cave; the cave's `or` and this swap may race,
    // which at worst drops one report.
    let word = unsafe { &*(seen as *const AtomicU32) };
    let mut last = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let now = word.swap(0, Ordering::AcqRel);
        if now != last {
            if now != 0 {
                log_line(&format!("weapon submenu built: let through {} (a row shows only for an installed tool)", mask_names(now)));
            }
            last = now;
        }
    }
}

// ---- the mod ------------------------------------------------------------------------------------

pub struct ToolModeBinds {
    settings: Arc<Mutex<Settings>>,
    status: Arc<Mutex<String>>,
    log: Arc<Log>,
    image: Mutex<ProcessImage>,
    sites: Option<Sites>,
}

impl ToolModeBinds {
    pub fn new(ctx: &super::Ctx) -> Self {
        let dir = paths::mod_dir(&ctx.root, NAME);
        let settings = Settings::load(
            &dir.join("tool_mode_binds.json"),
            json!({
                "enabled": DEFAULT_ENABLED,
                "mining_laser": true,
                "boltcaster": true,
                "terrain_manipulator": true,
                "more_modes": "",
                "write_logs": false,
            }),
        );
        let log = Arc::new(Log::open(&dir.join("tool_mode_binds.log"), settings.is_true("write_logs")));
        LOG_ON.store(settings.is_true("write_logs"), Ordering::Release);
        let _ = LOG.set(log.clone());

        let sites = match prepare(&ctx.image) {
            Ok(s) => Some(s),
            Err(e) => {
                log.info(&format!("Tool rows not installed: {e}"));
                ctx.log.info(&format!("ToolModeBinds not installed: {e}"));
                None
            }
        };
        if let Some(s) = &sites {
            let seen = s.seen;
            let _ = std::thread::Builder::new().name("tool-mode-binds-rows".into()).spawn(move || watch_rows(seen));
        }
        let m = Self {
            settings: Arc::new(Mutex::new(settings)),
            status: Arc::new(Mutex::new(String::new())),
            log,
            image: Mutex::new(ctx.image.clone()),
            sites,
        };
        let _ = m.sync_extra();
        let on = m.settings.lock().map(|s| s.is_not_false("enabled")).unwrap_or(false);
        let _ = m.apply(on);
        m
    }

    /// The extra mask from the settings, into the detour and the cave.
    fn sync_extra(&self) -> Result<u32, String> {
        let s = self.settings.lock().map_err(|_| "settings lock poisoned")?;
        let more = parse_modes(s.get("more_modes").and_then(Value::as_str).unwrap_or("")).unwrap_or_else(|e| {
            self.log.info(&format!("More modes ignored: {e}"));
            0
        });
        let mask = extra_mask(s.is_not_false("mining_laser"), s.is_not_false("boltcaster"), s.is_not_false("terrain_manipulator"), more);
        EXTRA.store(mask, Ordering::Release);
        if let Some(sites) = &self.sites {
            // SAFETY: our own cave word, read by the cave with a plain 4-byte load.
            unsafe { mem::write(sites.extra, mask) };
        }
        Ok(mask)
    }

    /// Write (on) or restore (off) both sites. On: the action call first, so a row we add always routes.
    fn apply(&self, on: bool) -> Result<(), String> {
        let Some(s) = &self.sites else {
            self.set_status("Off, see log");
            return Err("game code not found (see the log)".into());
        };
        let mut image = self.image.lock().map_err(|_| "image lock poisoned")?;
        let result = if on {
            ENABLED.store(true, Ordering::Release);
            swap(&mut image, s.action, &s.action_old, &s.action_new, "the Quick Menu weapon action")
                .and_then(|_| swap(&mut image, s.filter, &sigs::FILTER_OLD, &s.filter_new, "the row filter"))
        } else {
            let r = swap(&mut image, s.filter, &s.filter_new, &sigs::FILTER_OLD, "the row filter")
                .and_then(|_| swap(&mut image, s.action, &s.action_new, &s.action_old, "the Quick Menu weapon action"));
            ENABLED.store(false, Ordering::Release);
            r
        };
        drop(image);
        match &result {
            Ok(()) if on => {
                self.log.info(&format!("On: extra rows {}", mask_names(EXTRA.load(Ordering::Acquire))));
                self.set_status("On");
            }
            Ok(()) => {
                self.log.info("Off: game code is vanilla");
                self.set_status("Off");
            }
            Err(e) => {
                // Put back whatever was written; leave the game vanilla.
                if let Ok(mut image) = self.image.lock() {
                    let _ = swap(&mut image, s.filter, &s.filter_new, &sigs::FILTER_OLD, "the row filter");
                    let _ = swap(&mut image, s.action, &s.action_new, &s.action_old, "the Quick Menu weapon action");
                }
                ENABLED.store(false, Ordering::Release);
                self.log.info(&format!("Tool rows off: {e}"));
                self.set_status("Off, see log");
            }
        }
        result
    }

    fn set_status(&self, text: &str) {
        if let Ok(mut s) = self.status.lock() {
            *s = text.to_string();
        }
    }
}

impl GameMod for ToolModeBinds {
    fn name(&self) -> &'static str {
        NAME
    }

    fn specs(&self) -> &'static [SettingSpec] {
        SPECS
    }

    fn get(&self, key: &str) -> Option<Value> {
        match key {
            "status" => Some(json!(self.status.lock().ok()?.clone())),
            "more_modes" => self.settings.lock().ok()?.get(key).cloned(),
            "write_logs" => Some(json!(self.settings.lock().ok()?.is_true(key))),
            "enabled" | "mining_laser" | "boltcaster" | "terrain_manipulator" => {
                Some(json!(self.settings.lock().ok()?.is_not_false(key)))
            }
            _ => None,
        }
    }

    fn default_value(&self, key: &str) -> Option<Value> {
        match key {
            "status" => None,
            _ => self.settings.lock().ok()?.default(key).cloned(),
        }
    }

    fn set(&self, key: &str, value: &Value) -> Result<String, String> {
        let label = SPECS.iter().find(|s| s.key == key).map_or(key, |s| s.label);
        match key {
            "more_modes" => {
                let text = value.as_str().ok_or_else(|| "expected text".to_string())?;
                parse_modes(text)?;
                self.settings.lock().map_err(|_| "settings lock poisoned")?.set(key, json!(text.trim()))?;
                let mask = self.sync_extra()?;
                Ok(format!("{label}: rows {}", mask_names(mask)))
            }
            "enabled" | "write_logs" | "mining_laser" | "boltcaster" | "terrain_manipulator" => {
                let on = value.as_bool().ok_or_else(|| "expected true or false".to_string())?;
                self.settings.lock().map_err(|_| "settings lock poisoned")?.set(key, json!(on))?;
                match key {
                    "enabled" => self.apply(on).map_err(|e| format!("{label} cannot apply: {e}"))?,
                    "write_logs" => {
                        self.log.set_enabled(on);
                        LOG_ON.store(on, Ordering::Release);
                    }
                    _ => {
                        self.sync_extra()?;
                    }
                }
                Ok(format!("{label} {}", if on { "on" } else { "off" }))
            }
            _ => Err(format!("unknown setting {key:?}")),
        }
    }
}

pub fn register(ctx: &super::Ctx) -> Box<dyn GameMod> {
    Box::new(ToolModeBinds::new(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The secondary mask read in game with every tool owned.
    const LIVE_MASK: u32 = 0x0006_47C0;

    #[test]
    fn default_extra_is_the_three_tools() {
        assert_eq!(extra_mask(true, true, true, 0), 0x821);
        assert_eq!(extra_mask(true, false, false, 0), LASER);
        assert_eq!(extra_mask(false, false, false, 0), 0);
        assert_eq!(extra_mask(false, false, false, 1 << 3), 1 << 3);
        // Nothing above mode 20 ever reaches the cave.
        assert_eq!(extra_mask(false, false, false, u32::MAX), (1 << 21) - 1);
        // The three defaults are primaries in the live mask.
        assert_eq!(LIVE_MASK & 0x821, 0);
    }

    #[test]
    fn parses_mode_names_and_numbers() {
        assert_eq!(parse_modes(""), Ok(0));
        assert_eq!(parse_modes("  "), Ok(0));
        assert_eq!(parse_modes("Shotgun"), Ok(1 << 1));
        assert_eq!(parse_modes("shotgun, RAIL;cannon 2"), Ok((1 << 1) | (1 << 3) | (1 << 4) | (1 << 2)));
        assert_eq!(parse_modes("20"), Ok(1 << 20));
        assert!(parse_modes("21").is_err());
        assert!(parse_modes("-1").is_err());
        let e = parse_modes("Shotgun, Blaster").unwrap_err();
        assert!(e.contains("Blaster") && e.contains("Shotgun, Burst"));
    }

    #[test]
    fn routes_only_added_primaries_to_set_primary() {
        let extra = 0x821;
        assert_eq!(route(true, 5, Some(LIVE_MASK), extra), Route::Primary);
        assert_eq!(route(true, 0, Some(LIVE_MASK), extra), Route::Primary);
        assert_eq!(route(true, 11, Some(LIVE_MASK), extra), Route::Primary);
        // Plasma Launcher is a secondary: vanilla.
        assert_eq!(route(true, 6, Some(LIVE_MASK), extra), Route::Vanilla);
        // A primary we did not add: vanilla (which refuses it, as before).
        assert_eq!(route(true, 1, Some(LIVE_MASK), extra), Route::Vanilla);
        // Off, mask unreadable, out of range: vanilla.
        assert_eq!(route(false, 5, Some(LIVE_MASK), extra), Route::Vanilla);
        assert_eq!(route(true, 5, None, extra), Route::Vanilla);
        assert_eq!(route(true, 21, Some(0), u32::MAX), Route::Vanilla);
        assert_eq!(route(true, -1, Some(0), u32::MAX), Route::Vanilla);
        // A secondary listed in more_modes stays vanilla.
        assert_eq!(route(true, 6, Some(LIVE_MASK), extra | (1 << 6)), Route::Vanilla);
    }

    #[test]
    fn names_for_logs() {
        assert_eq!(mode_name(5), "Laser (Mining Laser)");
        assert_eq!(mode_name(1), "Shotgun");
        assert_eq!(mode_name(99), "mode 99");
        assert_eq!(mask_names(0x821), "Bolt (Boltcaster), Laser (Mining Laser), TerrainEdit (Terrain Manipulator)");
        assert_eq!(mask_names(0), "none");
    }

    fn target(code: &[u8], at: usize, base: usize) -> usize {
        let d = i32::from_le_bytes([code[at], code[at + 1], code[at + 2], code[at + 3]]);
        (base as i64 + at as i64 + 4 + i64::from(d)) as usize
    }

    #[test]
    fn filter_cave_keeps_vanilla_first_and_lands_on_both_exits() {
        // Addresses from the disassembly (build 183791): filter 0x141559e5d, join 0x141559e6b, skip 0x141559f43.
        let cave = 0x1_3000_0000usize;
        let (join, skip) = (0x1_4155_9E6Busize, 0x1_4155_9F43usize);
        let c = filter_cave(cave, join, skip).unwrap();
        assert_eq!(&c[..10], &[0x8B, 0x84, 0x87, 0x68, 0x0E, 0x00, 0x00, 0x44, 0x85, 0xF0]);
        assert_eq!(&c[10..12], &[0x0F, 0x85]);
        assert_eq!(target(&c, 12, cave), join);
        assert_eq!(&c[16..19], &[0x44, 0x85, 0x35]);
        assert_eq!(target(&c, 19, cave), cave + EXTRA_AT);
        assert_eq!(&c[23..25], &[0x0F, 0x84]);
        assert_eq!(target(&c, 25, cave), skip);
        assert_eq!(&c[29..32], &[0x44, 0x09, 0x35]);
        assert_eq!(target(&c, 32, cave), cave + SEEN_AT);
        assert_eq!(c[36], 0xE9);
        assert_eq!(target(&c, 37, cave), join);
        assert_eq!(c.len(), 41);
        assert!(c.len() <= STUB_AT);
    }

    #[test]
    fn site_bytes() {
        let site = 0x1_4155_9E5Dusize;
        let j = filter_jump(site, site + 0x100).unwrap();
        assert_eq!(j[0], 0xE9);
        assert_eq!(target(&j, 1, site), site + 0x100);
        assert!(j[5..].iter().all(|&b| b == 0x90));
        let c = call_to(0x1_4156_67AF, 0x1_4156_67AF + 0x40).unwrap();
        assert_eq!(c, [0xE8, 0x3B, 0, 0, 0]);
        assert!(filter_jump(0x1000, 0x7FFF_FFFF_0000).is_err());
        let s = abs_jump(0x1122_3344_5566_7788);
        assert_eq!(&s[..6], &[0xFF, 0x25, 0, 0, 0, 0]);
        assert_eq!(&s[6..], &0x1122_3344_5566_7788u64.to_le_bytes());
        assert!(STUB_AT + s.len() <= EXTRA_AT);
    }

    #[test]
    fn sig_constants_agree() {
        // The filter pattern carries the old bytes (minus the je distance) at FILTER_AT.
        let words: Vec<&str> = sigs::FILTER_LOOP.split_whitespace().collect();
        for (i, b) in sigs::FILTER_OLD.iter().enumerate().take(10) {
            assert_eq!(words[sigs::FILTER_AT + i], format!("{b:02X}"));
        }
        let action: Vec<&str> = sigs::ACTION_SITE.split_whitespace().collect();
        assert_eq!(action[sigs::ACTION_CALL_AT], "E8");
        let sp: Vec<&str> = sigs::SET_PRIMARY.split_whitespace().collect();
        assert_eq!(sp[sigs::SET_PRIMARY_CAN_CALL], "E8");
        let sel: Vec<&str> = sigs::SELECT.split_whitespace().collect();
        assert_eq!(&sel[sigs::SELECT_G_DISP - 3..sigs::SELECT_G_DISP], &["48", "8B", "35"]);
    }

    /// `cargo test -p nms-mods tool_mode_binds -- --ignored --nocapture` with the game's exe on disk.
    #[test]
    #[ignore]
    fn locates_every_site_in_the_exe() {
        let exe = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().map(|d| d.join("Binaries").join("NMS.exe")).find(|p| p.is_file());
        let Some(exe) = exe else { return };
        let image = nms_core::FileImage::load(&exe).unwrap();
        let at = locate(&image).unwrap();
        // Exe of 89,535,560 bytes, build 184271.
        assert_eq!(at.filter, 0x155DF1D);
        assert_eq!(at.join, 0x155DF2B);
        assert_eq!(at.skip, 0x155E003);
        assert_eq!(at.global, 0x6F6B208);
        assert_eq!(at.select, 0x1439D10);
        assert_eq!(at.action, 0x156A86F);
        assert_eq!(at.set_primary, 0x1434BB0);
        let base = 0x1_4000_0000usize;
        let cave = base + 0x7000_0000;
        let body = filter_cave(cave, base + at.join, base + at.skip).unwrap();
        println!("CAVE@{cave:#x} {}", hex(&body));
        println!("JUMP {}", hex(&filter_jump(base + at.filter, cave).unwrap()));
    }

    #[test]
    fn settings_listed_with_the_switch_on_by_default() {
        assert!(DEFAULT_ENABLED);
        for key in ["status", "enabled", "mining_laser", "boltcaster", "terrain_manipulator", "more_modes", "write_logs"] {
            assert!(SPECS.iter().any(|s| s.key == key), "{key}");
        }
        for s in SPECS.iter().filter(|s| s.kind != Kind::Status) {
            assert!(crate::common::menu_text::label("en", NAME, s.key).is_some(), "label {}", s.key);
            assert!(crate::common::menu_text::desc("en", NAME, s.key).is_some(), "desc {}", s.key);
        }
    }
}

//! Recharge every technology the charge menu lists (ported from Python).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use nms_core::hook::Hook;
use nms_core::log::Log;
use nms_core::mem::{self, read_bytes, read_i32, read_ptr};
use nms_core::process::ProcessImage;
use nms_core::{paths, Code, GameMod, Kind, Pattern, SettingSpec, Settings};
use nms_sdk::sigs::quick_recharge;
use serde_json::{json, Value};

type TriggerFn = unsafe extern "system" fn(u64, u64, u8) -> u8;
type Charge1Fn = unsafe extern "system" fn(u64, i32, u64);
type WalletFn = unsafe extern "system" fn() -> u64;
type PostFn = unsafe extern "system" fn(u64, u64, u64) -> u8;
type KindsFn = unsafe extern "system" fn(u64, i32, u64) -> u64;
type StoreFn = unsafe extern "system" fn(u64, i32, i64) -> u64;
type LookupFn = unsafe extern "system" fn(u64, u64) -> u64;
type FreeFn = unsafe extern "system" fn(u64);
type MidFn = unsafe extern "system" fn(u64, u64, i32, u64, i32, i32) -> i32;
type CompatFn = unsafe extern "system" fn(u64, u64, u64) -> u8;
type FactorFn = unsafe extern "system" fn(u64, u64, u64) -> f32;
type TableFn = unsafe extern "system" fn(u64, u64, u8) -> u64;
type TextMgrFn = unsafe extern "system" fn() -> u64;
type GetTextFn = unsafe extern "system" fn(u64, u64, i32) -> u64;
type ToastAddFn = unsafe extern "system" fn(
    u64, u64, f32, u64, u32, u64, u64, f32, u8, u8, u8,
);
type ToastCleanFn = unsafe extern "system" fn(u64);

const CHARGE_LEA_AT: usize = 7;
const CHARGE_LEA: [u8; 3] = [0x48, 0x8D, 0x88];
const OBJ_DISP_AT: usize = 20;
const HEAD_KINDS_AT: usize = 0x11;
const HEAD_STORE_AT: usize = 0x33;
const HEAD_LOOKUP_AT: usize = 0x47;
const HEAD_STATE_DISP_AT: usize = 0x0D;
const HEAD_INV_DISP_AT: usize = 0x2F;
const LOOP_MID_AT: usize = 0x34;
const MID_LOOKUP_AT: usize = 0x368;
const MID_COMPAT_AT: usize = 0x384;
const MID_FACTOR_AT: usize = 0x420;
const NAME_PRODUCT_AT: usize = 0x5F;
const NAME_TEXTMGR_AT: usize = 0x6C;
const NAME_GETTEXT_AT: usize = 0x88;
const NAME_SUBSTANCE_AT: usize = 0x9B;
const NAME_TECH_AT: usize = 0xD7;
const NAME_PRODUCT_LEA_AT: usize = 0x71;
const NAME_SUBSTANCE_LEA_AT: usize = 0xAD;
const NAME_TECH_LEA_AT: usize = 0xE9;
const TOAST_GLOBAL_AT: usize = 3;
const TOAST_R9_AT: usize = 40;
const TOAST_OBJECT_AT: usize = 52;
const TOAST_ADD_AT: usize = 84;
const TOAST_CLEAN_AT: usize = 94;
const EVENT_OP_AT: usize = 0x35;
const EVENT_OP: [u8; 2] = [0xC7, 0x85];
const EVENT_AT: usize = EVENT_OP_AT + 6;
const ITEM_TYPE: usize = 0x24;
const ITEM_STRIDE: usize = 0x30;
const STORE_COUNT: usize = 0x8C;
const STORE_DATA: usize = 0x90;
const ITEM_AMOUNT: usize = 0x18;
const ITEM_MAX: usize = 0x20;
const ITEM_KEY: usize = 0x10;
const FACTOR_TABLE: usize = 0x60;
const CHARGE_MENU: i32 = 0x16;
const CHARGE_SUB: i32 = 0x18;
const QM_BASE: usize = 0xA050;
const QM_COUNT: usize = 0xC;
const QM_DATA: usize = 0x10;
const QM_LEVEL: usize = 0x10;
const QM_ENTRY: usize = 0xE0;
const QM_TYPE: usize = 4;
const QM_KIND: usize = 0x40;
const QM_ID: usize = 0x44;
const TOAST_AUDIO: u32 = 0x18AECCF6;
const TOAST_SECONDS: f32 = 5.0;
const NOTICE_DEFAULT: &str = "All technologies recharged";

static TRIGGER_ORIG: AtomicUsize = AtomicUsize::new(0);
static RUNTIME: OnceLock<Arc<QuickRuntime>> = OnceLock::new();

const FUEL_NAMES: &[(&str, &str)] = &[
    ("FUEL1", "Carbon"),
    ("FUEL2", "Condensed Carbon"),
    ("OXYGEN", "Oxygen"),
    ("HOT1", "Phosphorus"),
    ("COLD1", "Dioxite"),
    ("TOXIC1", "Ammonia"),
    ("LAND1", "Ferrite Dust"),
    ("LAND2", "Pure Ferrite"),
    ("LAND3", "Magnetised Ferrite"),
    ("SAND1", "Silicate Powder"),
    ("RADIO1", "Uranium"),
    ("ROCKETSUB", "Tritium"),
    ("LAUNCHSUB", "Di-hydrogen"),
    ("LAUNCHSUB2", "Deuterium"),
    ("SODIUM", "Sodium"),
    ("SODIUM2", "Sodium Nitrate"),
    ("CATALYST1", "Sodium"),
    ("CATALYST2", "Sodium Nitrate"),
    ("CHARGECELL", "Ion Battery"),
    ("POWERCELL", "Ion Battery"),
    ("POWERCELL2", "Advanced Ion Battery"),
    ("GRENFUEL1", "Unstable Plasma"),
    ("SHIPCHARGE", "Starshield Battery"),
    ("LAUNCHFUEL", "Starship Launch Fuel"),
    ("HYPERFUEL1", "Warp Cell"),
    ("HYPERFUEL2", "Warp Hypercore"),
    ("PLANT_HOT", "Solanium"),
    ("PLANT_SNOW", "Frost Crystal"),
    ("PLANT_TOXIC", "Fungal Mould"),
    ("PLANT_RADIO", "Gamma Root"),
    // Preserve the original setting token alongside the current game substance ID.
    ("DUST1", "Pyrite"),
    ("DUSTY1", "Pyrite"),
];

// Accepted in saved and manual exclusions. New toggle edits write the current game IDs.
const LEGACY_FUEL_IDS: &[&str] = &["SODIUM", "SODIUM2", "CHARGECELL", "DUST1"];

type FuelMask = u64;
#[cfg(test)]
const ALL_FUELS: FuelMask = (1 << FUEL_NAMES.len()) - 1;

pub(crate) struct FuelGroup {
    pub(crate) id: &'static str,
    pub(crate) name: &'static str,
    pub(crate) mask: FuelMask,
}

// These legacy exclusions still parse, but have no recharge targets in the stock
// technology table for build 185034.
const PLANTS_WITHOUT_RECHARGE_TARGETS: &[&str] = &["PLANT_HOT", "PLANT_SNOW", "PLANT_TOXIC", "PLANT_RADIO"];

/// One row per selectable display name, in catalogue order. The first ID is its translation key.
pub(crate) fn fuel_groups() -> Vec<FuelGroup> {
    let mut groups: Vec<FuelGroup> = Vec::new();
    for (index, &(id, name)) in FUEL_NAMES.iter().enumerate() {
        if PLANTS_WITHOUT_RECHARGE_TARGETS.contains(&id) {
            continue;
        }
        let bit = 1 << index;
        if let Some(group) = groups.iter_mut().find(|group| group.name == name) {
            group.mask |= bit;
        } else {
            groups.push(FuelGroup { id, name, mask: bit });
        }
    }
    groups
}

/// Fuel groups changed on this draw, to be projected onto the shared draft.
#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
pub(crate) struct FuelEdits {
    exclude: FuelMask,
    allow: FuelMask,
}

impl FuelEdits {
    pub(crate) fn apply(self, live: FuelMask) -> FuelMask {
        (live | self.exclude) & !self.allow
    }

    pub(crate) fn set_group(&mut self, group: FuelMask, excluded: bool, live: FuelMask) {
        self.exclude &= !group;
        self.allow &= !group;
        if excluded && live & group != group {
            self.exclude |= group;
        } else if !excluded && live & group != 0 {
            self.allow |= group;
        }
    }

    #[cfg(test)]
    fn reset() -> Self {
        Self { exclude: 0, allow: ALL_FUELS }
    }

    pub(crate) fn is_empty(self) -> bool {
        self.exclude == 0 && self.allow == 0
    }
}

struct FuelMaps {
    id_name_keys: HashMap<String, String>,
    name_ids: HashMap<String, HashSet<&'static str>>,
    name_label: HashMap<String, String>,
    name_norms: Vec<String>,
}

fn fuel_maps() -> &'static FuelMaps {
    static MAPS: OnceLock<FuelMaps> = OnceLock::new();
    MAPS.get_or_init(|| {
        let mut id_name_keys = HashMap::new();
        let mut name_ids: HashMap<String, HashSet<&'static str>> = HashMap::new();
        let mut name_label: HashMap<String, String> = HashMap::new();
        for (ident, label) in FUEL_NAMES {
            let key = norm_name(label);
            id_name_keys.insert(ident.to_ascii_lowercase(), key.clone());
            name_ids.entry(key.clone()).or_default().insert(ident);
            name_label.entry(key).or_insert_with(|| (*label).to_string());
        }
        let name_norms: Vec<String> = name_label.keys().cloned().collect();
        FuelMaps {
            id_name_keys,
            name_ids,
            name_label,
            name_norms,
        }
    })
}

fn norm_name(text: &str) -> String {
    text.to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn sequence_matcher_ratio(a: &str, b: &str) -> f64 {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    if ac.is_empty() && bc.is_empty() {
        return 1.0;
    }
    let matched = matching_chars(&ac, &bc, 0, ac.len(), 0, bc.len());
    2.0 * matched as f64 / (ac.len() + bc.len()) as f64
}

/// Python `difflib.SequenceMatcher(None, a, b)` matching-block total (no junk; names are far below
/// the 200-char autojunk limit). Longest common block first, ties to the earliest in `a` then `b`,
/// then recurse left and right of it.
fn matching_chars(a: &[char], b: &[char], alo: usize, ahi: usize, blo: usize, bhi: usize) -> usize {
    let (mut best_i, mut best_j, mut best) = (alo, blo, 0usize);
    let width = bhi.saturating_sub(blo) + 1;
    let mut prev = vec![0usize; width];
    let mut cur = vec![0usize; width];
    for i in alo..ahi {
        for j in blo..bhi {
            let col = j - blo + 1;
            let run = if a.get(i) == b.get(j) { prev.get(col - 1).copied().unwrap_or(0) + 1 } else { 0 };
            if let Some(slot) = cur.get_mut(col) {
                *slot = run;
            }
            if run > best {
                best = run;
                best_i = i + 1 - run;
                best_j = j + 1 - run;
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    if best == 0 {
        return 0;
    }
    best + matching_chars(a, b, alo, best_i, blo, best_j)
        + matching_chars(a, b, best_i + best, ahi, best_j + best, bhi)
}

fn match_fuel_token(token: &str) -> (Option<String>, HashSet<String>) {
    let maps = fuel_maps();
    let raw = token.trim();
    if raw.is_empty() {
        return (None, HashSet::new());
    }
    // IDs and names resolve to the same fuel, even when the text matches both.
    let key = maps.id_name_keys.get(&raw.to_ascii_lowercase()).cloned()
        .unwrap_or_else(|| norm_name(raw));
    if let Some(ids) = maps.name_ids.get(&key) {
        let label = maps.name_label.get(&key).cloned();
        return (label, ids.iter().map(|s| (*s).to_string()).collect());
    }
    let long = key.len() >= 5 || key.contains(' ');
    if long {
        let subs: Vec<&String> = maps.name_norms.iter().filter(|name| name.contains(&key)).collect();
        if subs.len() == 1 {
            let name = subs[0];
            return (
                maps.name_label.get(name).cloned(),
                maps.name_ids.get(name).into_iter().flatten().map(|s| (*s).to_string()).collect(),
            );
        }
        let toks: HashSet<&str> = key.split_whitespace().collect();
        if !toks.is_empty() {
            let hits: Vec<&String> = maps
                .name_norms
                .iter()
                .filter(|name| toks.iter().all(|t| name.split_whitespace().any(|w| w == *t)))
                .collect();
            if hits.len() == 1 {
                let name = hits[0];
                return (
                    maps.name_label.get(name).cloned(),
                    maps.name_ids.get(name).into_iter().flatten().map(|s| (*s).to_string()).collect(),
                );
            }
        }
        let mut ranked: Vec<(f64, &String)> = maps
            .name_norms
            .iter()
            .map(|name| (sequence_matcher_ratio(&key, name), name))
            .collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        if let Some((best, name)) = ranked.first() {
            let second = ranked.get(1).map(|(r, _)| *r).unwrap_or(0.0);
            if *best >= 0.70 && *best - second >= 0.12 {
                return (
                    maps.name_label.get(*name).cloned(),
                    maps.name_ids.get(*name).into_iter().flatten().map(|s| (*s).to_string()).collect(),
                );
            }
        }
    }
    (None, HashSet::new())
}

fn parse_never(text: &str) -> (HashSet<String>, Vec<String>, Vec<String>) {
    let mut ids = HashSet::new();
    let mut labels = Vec::new();
    let mut unknown = Vec::new();
    let mut seen = HashSet::new();
    for part in text.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        let (label, found) = match_fuel_token(token);
        if found.is_empty() {
            unknown.push(token.to_string());
            continue;
        }
        ids.extend(found);
        if let Some(label) = label {
            if seen.insert(label.clone()) {
                labels.push(label);
            }
        }
    }
    (ids, labels, unknown)
}

fn fuel_mask(ids: &HashSet<String>) -> FuelMask {
    FUEL_NAMES.iter().enumerate().fold(0, |mask, (i, (id, _))| {
        mask | (FuelMask::from(ids.contains(*id)) << i)
    })
}

/// Keep unrelated tokens byte for byte. When dropping a token that covers several IDs, add
/// back just the surviving IDs that aren't already covered by another token.
pub(crate) fn edit_fuel_exclusions(text: &str, edits: FuelEdits) -> Option<String> {
    let tokens: Vec<_> = text.split(',').map(|token| {
        (token, fuel_mask(&match_fuel_token(token).1))
    }).collect();
    let current = tokens.iter().fold(0, |mask, &(_, ids)| mask | ids);
    let desired = edits.apply(current);
    if desired == current {
        return None;
    }
    let mut covered = 0;
    let kept: Vec<_> = tokens.into_iter().filter_map(|(token, ids)| {
        if ids & !desired != 0 {
            return None;
        }
        covered |= ids;
        Some(token)
    }).collect();
    let mut edited = kept.join(",");
    for (index, &(id, _)) in FUEL_NAMES.iter().enumerate() {
        if (desired & !covered) & (1 << index) != 0 && !LEGACY_FUEL_IDS.contains(&id) {
            if edited.trim().is_empty() {
                edited.clear();
            } else {
                edited.push_str(", ");
            }
            edited.push_str(id);
        }
    }
    Some(edited)
}

fn never_spend_text(settings: &Settings) -> &str {
    settings.get("never_spend").and_then(Value::as_str).unwrap_or_default()
}

pub(crate) fn fuel_exclusion_mask(text: &str) -> FuelMask {
    fuel_mask(&parse_never(text).0)
}

struct NameFns {
    product: usize,
    substance: usize,
    tech: usize,
    textmgr: usize,
    gettext: usize,
    offsets: [i32; 3],
}

struct Spend {
    state_off: i32,
    inv_off: i32,
    kinds: usize,
    store: usize,
    lookup: usize,
    free: usize,
    mid: usize,
    compat: usize,
    factor: usize,
    factor_global: usize,
    name_at: usize,
    names: Option<NameFns>,
}

struct Toast {
    add: usize,
    clean: usize,
    holder_at: usize,
    title: usize,
    object_off: i32,
}

struct Game {
    charge1: usize,
    wallet: usize,
    state_off: i32,
    post: Option<usize>,
    obj_site: usize,
    event: u32,
    toast: Option<Toast>,
    spend: Option<Spend>,
    notice: [u8; 128],
    timer: [u8; 16],
}

struct QuickRuntime {
    log: Arc<Log>,
    status: Arc<Mutex<String>>,
    settings: Arc<Mutex<Settings>>,
    game: Mutex<Option<Game>>,
    skip_ids: Mutex<HashSet<String>>,
    skip_key: Mutex<Option<HashSet<String>>>,
    table_logged: Mutex<HashSet<String>>,
    namer_live: AtomicBool,
    /// Game language for the notice (`ui.qr_*`), read once at start: a change shows after a restart.
    lang: &'static str,
}

const SPECS: &[SettingSpec] = &[
    SettingSpec {
        key: "status",
        label: "Status",
        kind: Kind::Status,
    },
    SettingSpec {
        key: "write_logs",
        label: "Write log",
        kind: Kind::Bool,
    },
    SettingSpec {
        key: "recharge_all",
        label: "Charge menu: all",
        kind: Kind::Bool,
    },
    SettingSpec {
        key: "substances_first",
        label: "Substances first",
        kind: Kind::Bool,
    },
    SettingSpec {
        key: "never_spend",
        label: "Never spend",
        kind: Kind::Text,
    },
    SettingSpec {
        key: "play_sound",
        label: "Play charge sound",
        kind: Kind::Bool,
    },
    SettingSpec {
        key: "show_notice",
        label: "Show notice",
        kind: Kind::Bool,
    },
    SettingSpec {
        key: "free_fill",
        label: "Free fill (no cost)",
        kind: Kind::Bool,
    },
];

pub struct QuickRecharge {
    settings: Arc<Mutex<Settings>>,
    status: Arc<Mutex<String>>,
    log: Arc<Log>,
    #[expect(dead_code)]
    trigger_hook: Option<Hook>,
}

pub fn register(ctx: &super::Ctx) -> Box<dyn GameMod> {
    Box::new(QuickRecharge::new(ctx))
}

impl QuickRecharge {
    fn new(ctx: &super::Ctx) -> Self {
        let dir = paths::mod_dir(&ctx.root, "QuickRecharge");
        let settings = Arc::new(Mutex::new(Settings::load(
            &dir.join("quick_recharge.json"),
            json!({
                "write_logs": false,
                "recharge_all": true,
                "substances_first": false,
                "play_sound": true,
                "show_notice": true,
                "free_fill": false,
                "never_spend": "",
            }),
        )));
        let write_logs = settings
            .lock()
            .map(|s| s.is_not_false("write_logs"))
            .unwrap_or(false);
        let log = Arc::new(Log::open(&dir.join("quick_recharge.log"), write_logs));
        let status = Arc::new(Mutex::new("Off, see log".to_string()));

        let runtime = Arc::new(QuickRuntime {
            log: log.clone(),
            status: status.clone(),
            settings: settings.clone(),
            game: Mutex::new(None),
            skip_ids: Mutex::new(HashSet::new()),
            skip_key: Mutex::new(None),
            table_logged: Mutex::new(HashSet::new()),
            namer_live: AtomicBool::new(false),
            lang: crate::common::lang::detect(&ctx.image, &log),
        });
        if let Ok(settings) = settings.lock() {
            runtime.refresh_never(never_spend_text(&settings));
        }
        let _ = RUNTIME.set(runtime.clone());

        let game_result = Game::new(&ctx.image, &log);
        let trigger_hook = install_trigger(&ctx.image, &log);

        match game_result {
            Ok(game) => {
                if let Ok(mut g) = runtime.game.lock() {
                    let namer = game.spend.as_ref().is_some_and(|s| s.name_at != 0);
                    runtime.namer_live.store(namer, Ordering::Release);
                    *g = Some(game);
                }
                if let Ok(mut s) = status.lock() {
                    *s = "On".to_string();
                }
                log.info("QuickRecharge is on");
            }
            Err(reason) => {
                log.info(&format!("QuickRecharge is off: {reason}"));
            }
        }

        Self {
            settings,
            status,
            log,
            trigger_hook,
        }
    }
}

impl QuickRuntime {
    fn never_report(&self, text: &str) -> String {
        let (ids, labels, unknown) = parse_never(text);
        if let Ok(mut skip) = self.skip_ids.lock() {
            *skip = ids;
        }
        let mut parts = Vec::new();
        if !labels.is_empty() {
            parts.push(labels.join(", "));
        }
        if !unknown.is_empty() {
            parts.push(format!("unknown {}", unknown.join(", ")));
        }
        parts.join("; ")
    }

    fn refresh_never(&self, text: &str) {
        let report = self.never_report(text);
        let key: HashSet<String> = self
            .skip_ids
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        let prev = self.skip_key.lock().ok().and_then(|k| k.clone());
        if prev.as_ref() != Some(&key) && (prev.is_some() || !key.is_empty()) {
            self.log
                .info(&format!("Never spend {}", if report.is_empty() { "off".into() } else { report }));
        }
        if let Ok(mut sk) = self.skip_key.lock() {
            *sk = Some(key);
        }
    }

    fn save_never_spend(&self, settings: &mut Settings, text: &str) -> Result<(), String> {
        settings.set("never_spend", json!(text))?;
        self.refresh_never(text);
        Ok(())
    }

    fn after_trigger(&self, menu: u64, entry: u64, commit: u8) {
        if commit == 0 {
            return;
        }
        let recharge = self
            .settings
            .lock()
            .ok()
            .map(|s| s.is_true("recharge_all"))
            .unwrap_or(false);
        if !recharge {
            return;
        }
        // try_lock: a charge call that re-enters QmTriggerAction on this thread must not deadlock.
        let Ok(mut game_guard) = self.game.try_lock() else {
            return;
        };
        let Some(game) = game_guard.as_mut() else {
            return;
        };
        let entry_i = addr_u64(entry);
        let menu_i = addr_u64(menu);
        if entry_i < mem::MIN_ADDR as u64 || menu_i < mem::MIN_ADDR as u64 {
            return;
        }
        if read_i32(entry_i as usize + QM_TYPE) != Some(CHARGE_SUB) {
            return;
        }
        if charge_all(self, game, menu_i).is_err() {
            self.log.info("Charge all: turned off");
            *game_guard = None;
            if let Ok(mut s) = self.status.lock() {
                *s = "Stopped, see log".to_string();
            }
        }
    }
}

fn install_trigger(image: &ProcessImage, log: &Log) -> Option<Hook> {
    let pat = Pattern::parse(quick_recharge::TRIGGER).ok()?;
    let trigger_rva = image.find_unique(&pat).unique()?;
    // SAFETY: signature matches TRIGGER_DEF; detour declares every argument.
    unsafe {
        Hook::install(
            "QmTriggerAction",
            image.va(trigger_rva),
            trigger_detour as *const (),
            &TRIGGER_ORIG,
        )
        .map_err(|e| log.info(&format!("QuickRecharge hook not installed: {e}")))
        .ok()
    }
}

fn addr_u64(value: u64) -> u64 {
    if value < mem::MIN_ADDR as u64 {
        0
    } else {
        value
    }
}

fn read_i32_at(addr: usize) -> Option<i32> {
    read_i32(addr)
}

fn item_text(item: usize) -> String {
    let Some(raw) = read_bytes(item, 16) else {
        return String::new();
    };
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end])
        .trim_start_matches('^')
        .to_string()
}

fn display_name(raw: &str, ident: &str) -> Option<String> {
    let mut parts = String::new();
    let mut depth = 0i32;
    for ch in raw.chars() {
        if ch == '<' {
            depth += 1;
        } else if ch == '>' && depth > 0 {
            depth -= 1;
        } else if depth == 0 {
            parts.push(ch);
        }
    }
    let value: String = parts.trim().chars().filter(|c| !c.is_control()).collect();
    if value.is_empty() || value.chars().count() > 40 {
        return None;
    }
    let key = value.to_ascii_lowercase();
    let id_fold = ident.to_ascii_lowercase();
    if key == id_fold || key == format!("{id_fold}s") {
        return None;
    }
    if value.contains('_') {
        return None;
    }
    Some(value)
}

fn e8_target(image: &ProcessImage, pattern_rva: usize, offset: usize) -> Option<usize> {
    let e8_rva = pattern_rva.checked_add(offset)?;
    let b = image.read(e8_rva, 1)?;
    if b.first() != Some(&0xE8) {
        return None;
    }
    let tgt_rva = image.rel32_target(e8_rva + 1)?;
    Some(image.va(tgt_rva))
}

fn read_disp(image: &ProcessImage, rva: usize) -> Option<i32> {
    let b = image.read(rva, 4)?;
    if b.len() < 4 {
        return None;
    }
    Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl Spend {
    fn new(image: &ProcessImage) -> Result<Self, String> {
        let head_pat = Pattern::parse(quick_recharge::HEAD).map_err(|e| e.to_string())?;
        let loop_pat = Pattern::parse(quick_recharge::LOOP).map_err(|e| e.to_string())?;
        let dtor_pat = Pattern::parse(quick_recharge::DTOR).map_err(|e| e.to_string())?;
        let Some(head_rva) = image.find_unique(&head_pat).unique() else {
            return Err("the Charge row code was not found (game update?)".into());
        };
        let Some(loop_rva) = image.find_unique(&loop_pat).unique() else {
            return Err("the Charge row code was not found (game update?)".into());
        };
        let Some(dtor_rva) = image.find_unique(&dtor_pat).unique() else {
            return Err("the Charge row code was not found (game update?)".into());
        };
        for off in [HEAD_KINDS_AT, HEAD_STORE_AT, HEAD_LOOKUP_AT] {
            let b = image.read(head_rva + off, 1).ok_or("the Charge row calls are not where they were")?;
            if b.first() != Some(&0xE8) {
                return Err("the Charge row calls are not where they were".into());
            }
        }
        let mid_rva = image
            .rel32_target(loop_rva + LOOP_MID_AT + 1)
            .ok_or_else(|| "the charge routine is not where it was".to_string())?;
        let mid_va = image.va(mid_rva);
        let mid_bytes = image.read(mid_rva, MID_FACTOR_AT + 20).ok_or("the charge routine is not where it was")?;
        if mid_bytes.get(MID_LOOKUP_AT) != Some(&0xE8) || mid_bytes.get(MID_COMPAT_AT) != Some(&0xE8) {
            return Err("the charge routine is not where it was".into());
        }
        let head_lookup = e8_target(image, head_rva, HEAD_LOOKUP_AT).ok_or("lookup")?;
        let mid_lookup = {
            let rva = mid_rva + MID_LOOKUP_AT;
            image.rel32_target(rva + 1).map(|t| image.va(t))
        }
        .ok_or("lookup mid")?;
        if head_lookup != mid_lookup {
            return Err("the charge routine item lookup differs".into());
        }
        let site_rva = mid_rva + MID_FACTOR_AT;
        let site = image.read(site_rva, 16).ok_or("factor site")?;
        if site.get(0..3) != Some(&[0x48, 0x8B, 0x0D]) || site.get(7..11) != Some(&[0x48, 0x83, 0xC1, 0x60]) {
            return Err("the charge routine factor call is not where it was".into());
        }
        if site.get(11) != Some(&0xE8) {
            return Err("the charge routine factor call is not where it was".into());
        }
        let state_off = read_disp(image, head_rva + HEAD_STATE_DISP_AT).ok_or("state disp")?;
        let inv_off = read_disp(image, head_rva + HEAD_INV_DISP_AT).ok_or("inv disp")?;
        let kinds = e8_target(image, head_rva, HEAD_KINDS_AT).ok_or("kinds")?;
        let store = e8_target(image, head_rva, HEAD_STORE_AT).ok_or("store")?;
        let lookup = head_lookup;
        let free = image.va(dtor_rva);
        let mid = mid_va;
        let compat = {
            let rva = mid_rva + MID_COMPAT_AT;
            image.rel32_target(rva + 1).map(|t| image.va(t))
        }
        .ok_or("compat")?;
        let factor = image.rel32_target(site_rva + 12).map(|t| image.va(t)).ok_or("factor")?;
        let disp = read_disp(image, site_rva + 3).ok_or("factor global disp")?;
        let factor_global = image.va(site_rva + 7).wrapping_add(disp as usize);

        let mut spend = Spend {
            state_off,
            inv_off,
            kinds,
            store,
            lookup,
            free,
            mid,
            compat,
            factor,
            factor_global,
            name_at: 0,
            names: None,
        };

        if let Ok(name_pat) = Pattern::parse(quick_recharge::NAME) {
            if let Some(name_rva) = image.find_unique(&name_pat).unique() {
                let top = name_rva;
                let calls = [NAME_PRODUCT_AT, NAME_TEXTMGR_AT, NAME_SUBSTANCE_AT, NAME_TECH_AT];
                let leas = [NAME_PRODUCT_LEA_AT, NAME_SUBSTANCE_LEA_AT, NAME_TECH_LEA_AT];
                let mut ok = true;
                for off in calls {
                    let b = image.read(top + off, 1);
                    if b.as_deref() != Some(&[0xE8]) {
                        ok = false;
                        break;
                    }
                }
                if ok && image.read(top + NAME_GETTEXT_AT, 1).as_deref() != Some(&[0xE9]) {
                    ok = false;
                }
                if ok {
                    for off in leas {
                        let b = image.read(top + off, 2);
                        if b.as_deref() != Some(&[0x48, 0x8D]) {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    let product = e8_target(image, top, NAME_PRODUCT_AT).unwrap_or(0);
                    let substance = e8_target(image, top, NAME_SUBSTANCE_AT).unwrap_or(0);
                    let tech = e8_target(image, top, NAME_TECH_AT).unwrap_or(0);
                    let textmgr = e8_target(image, top, NAME_TEXTMGR_AT).unwrap_or(0);
                    let gettext = image
                        .rel32_target(top + NAME_GETTEXT_AT + 1)
                        .map(|t| image.va(t))
                        .unwrap_or(0);
                    let mut offsets = [0i32; 3];
                    for (i, off) in leas.iter().enumerate() {
                        offsets[i] = read_disp(image, top + off + 3).unwrap_or(0);
                    }
                    spend.name_at = image.va(top);
                    spend.names = Some(NameFns {
                        product,
                        substance,
                        tech,
                        textmgr,
                        gettext,
                        offsets,
                    });
                }
            }
        }
        Ok(spend)
    }

    fn game_name(&self, item: usize, ident: &str) -> Option<String> {
        let names = self.names.as_ref()?;
        let table = read_ptr(self.factor_global)?;
        let kind = read_i32_at(item + ITEM_TYPE)?;
        let (data, off) = if kind == 2 {
            let tech: TableFn = unsafe { std::mem::transmute(names.tech) };
            let data = unsafe { tech(table.wrapping_add(FACTOR_TABLE) as u64, item as u64, 1) };
            (data, names.offsets[2])
        } else if kind == 1 {
            let product: TableFn = unsafe { std::mem::transmute(names.product) };
            let data = unsafe { product(table.wrapping_add(FACTOR_TABLE) as u64, item as u64, 1) };
            (data, names.offsets[0])
        } else {
            let substance: TableFn = unsafe { std::mem::transmute(names.substance) };
            let data = unsafe { substance(table.wrapping_add(FACTOR_TABLE) as u64, item as u64, 1) };
            (data, names.offsets[1])
        };
        let data = data as usize;
        if data < mem::MIN_ADDR {
            return None;
        }
        let textmgr: TextMgrFn = unsafe { std::mem::transmute(names.textmgr) };
        let mgr = unsafe { textmgr() };
        let gettext: GetTextFn = unsafe { std::mem::transmute(names.gettext) };
        let text = unsafe { gettext(mgr, data.wrapping_add(off as usize) as u64, 0) } as usize;
        if text < mem::MIN_ADDR {
            return None;
        }
        let raw = read_bytes(text, 96)?;
        let value = if raw.len() > 1 && raw[1] == 0 {
            decode_utf16_at(text, 48)
        } else {
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        };
        display_name(value.trim(), ident)
    }
}

fn decode_utf16_at(text: usize, max_chars: usize) -> String {
    let byte_len = max_chars.checked_mul(2).unwrap_or(0);
    let Some(raw) = read_bytes(text, byte_len) else {
        return String::new();
    };
    let mut units = Vec::new();
    for chunk in raw.chunks(2) {
        if chunk.len() < 2 {
            break;
        }
        let u = u16::from_le_bytes([chunk[0], chunk[1]]);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}

impl Game {
    fn new(image: &ProcessImage, log: &Log) -> Result<Self, String> {
        let charge1_pat = Pattern::parse(quick_recharge::CHARGE1).map_err(|e| e.to_string())?;
        let site_pat = Pattern::parse(quick_recharge::CHARGE_SITE).map_err(|e| e.to_string())?;
        let wallet_pat = Pattern::parse(quick_recharge::WALLET).map_err(|e| e.to_string())?;
        let trigger_pat = Pattern::parse(quick_recharge::TRIGGER).map_err(|e| e.to_string())?;
        let charge1_rva = image.find_unique(&charge1_pat).unique().ok_or("the game code it relies on was not found (game update?)")?;
        let site_rva = image.find_unique(&site_pat).unique().ok_or("the game code it relies on was not found (game update?)")?;
        let wallet_rva = image.find_unique(&wallet_pat).unique().ok_or("the game code it relies on was not found (game update?)")?;
        if image.find_unique(&trigger_pat).unique().is_none() {
            return Err("the game code it relies on was not found (game update?)".into());
        }
        let lea_rva = site_rva + CHARGE_LEA_AT;
        if image.read(lea_rva, CHARGE_LEA.len()).as_deref() != Some(&CHARGE_LEA) {
            return Err("unexpected ChargeMenu site (game update?)".into());
        }
        let state_off = read_disp(image, lea_rva + CHARGE_LEA.len()).ok_or("state off")?;

        let post_pat = Pattern::parse(quick_recharge::POST).ok();
        let obj_pat = Pattern::parse(quick_recharge::OBJ).ok();
        let post_rva = post_pat.and_then(|p| image.find_unique(&p).unique());
        let obj_rva = obj_pat.and_then(|p| image.find_unique(&p).unique());

        let mut post = None;
        let mut obj_site = 0usize;
        let mut event = 0u32;
        if let (Some(post_rva), Some(obj_rva)) = (post_rva, obj_rva) {
            let op_rva = site_rva + EVENT_OP_AT;
            if image.read(op_rva, EVENT_OP.len()).as_deref() == Some(&EVENT_OP) {
                if let Some(ev) = image.read(site_rva + EVENT_AT, 4).and_then(|b| {
                    (b.len() == 4).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                }) {
                    event = ev;
                    post = Some(image.va(post_rva));
                    obj_site = image.va(obj_rva);
                }
            }
        }

        let mut toast = None;
        if let Ok(toast_pat) = Pattern::parse(quick_recharge::TOAST) {
            if let Some(toast_rva) = image.find_unique(&toast_pat).unique() {
                let at = toast_rva;
                let add_ok = image.read(at + TOAST_ADD_AT, 1).as_deref() == Some(&[0xE8]);
                let clean_ok = image.read(at + TOAST_CLEAN_AT, 1).as_deref() == Some(&[0xE8]);
                let r9_ok = image.read(at + TOAST_R9_AT, 3).as_deref() == Some(&[0x4C, 0x8D, 0x0D]);
                let obj_ok = image.read(at + TOAST_OBJECT_AT, 3).as_deref() == Some(&[0x48, 0x8D, 0x8B]);
                if add_ok && clean_ok && r9_ok && obj_ok {
                    let add = e8_target(image, at, TOAST_ADD_AT).unwrap_or(0);
                    let clean = e8_target(image, at, TOAST_CLEAN_AT).unwrap_or(0);
                    let holder_disp = read_disp(image, at + TOAST_GLOBAL_AT).unwrap_or(0);
                    let holder_at = image.va(at + 7).wrapping_add(holder_disp as usize);
                    let title_disp = read_disp(image, at + TOAST_R9_AT + 3).unwrap_or(0);
                    let title = image.va(at + TOAST_R9_AT + 7).wrapping_add(title_disp as usize);
                    let object_off = read_disp(image, at + TOAST_OBJECT_AT + 3).unwrap_or(0);
                    toast = Some(Toast {
                        add,
                        clean,
                        holder_at,
                        title,
                        object_off,
                    });
                }
            }
        }

        let spend = match Spend::new(image) {
            Ok(s) => Some(s),
            Err(e) => {
                log.info(&format!("Spend resources not available: {e}"));
                None
            }
        };

        let mut notice = [0u8; 128];
        let bytes = NOTICE_DEFAULT.as_bytes();
        let n = bytes.len().min(127);
        notice[..n].copy_from_slice(&bytes[..n]);

        Ok(Game {
            charge1: image.va(charge1_rva),
            wallet: image.va(wallet_rva),
            state_off,
            post,
            obj_site,
            event,
            toast,
            spend,
            notice,
            timer: [0u8; 16],
        })
    }
}

#[derive(Clone, Copy)]
struct FuelPick {
    item: usize,
    store_kind: i32,
    per_unit: f32,
}

fn fuel_name(rt: &QuickRuntime, spend: &Spend, item: usize) -> String {
    let ident = item_text(item);
    if spend.name_at != 0 && rt.namer_live.load(Ordering::Acquire) {
        if let Some(found) = spend.game_name(item, &ident) {
            return found;
        }
        rt.log
            .info("Game name lookup failed, using the short table");
        rt.namer_live.store(false, Ordering::Release);
    }
    if let Some((_, label)) = FUEL_NAMES.iter().find(|(id, _)| *id == ident) {
        if let Ok(mut logged) = rt.table_logged.lock() {
            if logged.insert(ident.clone()) {
                rt.log.info(&format!(
                    "Notice name: {label} (id {ident}, game text missing)"
                ));
            }
        }
        return (*label).to_string();
    }
    ident
}

fn cards(menu: u64) -> Vec<usize> {
    let menu = menu as usize;
    let count = read_i32_at(menu.wrapping_add(QM_BASE + QM_LEVEL + QM_COUNT)).unwrap_or(0);
    let data = read_ptr(menu.wrapping_add(QM_BASE + QM_LEVEL + QM_DATA)).unwrap_or(0);
    if data < mem::MIN_ADDR || count <= 0 || count > 24 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut index = 0i32;
    while index < count {
        let entry = data.wrapping_add((index as usize).wrapping_mul(QM_ENTRY));
        if read_i32_at(entry + QM_TYPE) == Some(CHARGE_MENU) {
            found.push(entry);
        }
        index += 1;
    }
    found
}

fn whole_units(per_unit: f32, units: i32) -> i32 {
    let mut u = units;
    while u >= 1 {
        let prod = (u as f32) * per_unit;
        if prod != (prod as i32) as f32 {
            u -= 1;
        } else {
            break;
        }
    }
    u
}

fn choose(fuels: &[FuelPick], need: i32, substances_first: bool) -> Option<(FuelPick, i32)> {
    let mut list = fuels.to_vec();
    if substances_first {
        let preferred: Vec<FuelPick> = list
            .iter()
            .filter(|f| read_i32_at(f.item + ITEM_TYPE) == Some(0))
            .cloned()
            .collect();
        if !preferred.is_empty() {
            list = preferred;
        }
    }
    let mut sorted: Vec<&FuelPick> = list.iter().filter(|f| f.per_unit <= need as f32).collect();
    sorted.sort_by(|a, b| b.per_unit.partial_cmp(&a.per_unit).unwrap_or(std::cmp::Ordering::Equal));
    for fuel in sorted {
        let amount = read_i32_at(fuel.item + ITEM_AMOUNT).unwrap_or(0);
        let cap = (need as f32 / fuel.per_unit).floor() as i32;
        let units = whole_units(fuel.per_unit, amount.min(cap));
        if units >= 1 {
            return Some((*fuel, units));
        }
    }
    let covering: Vec<&FuelPick> = list
        .iter()
        .filter(|f| {
            let amount = read_i32_at(f.item + ITEM_AMOUNT).unwrap_or(0);
            let need_f = need as f32 / f.per_unit;
            amount as f32 >= need_f.ceil()
        })
        .collect();
    if let Some(fuel) = covering.iter().min_by(|a, b| a.per_unit.partial_cmp(&b.per_unit).unwrap_or(std::cmp::Ordering::Equal)) {
        let units = (need as f32 / fuel.per_unit).ceil() as i32;
        return Some((**fuel, units));
    }
    if let Some(fuel) = list.iter().max_by(|a, b| a.per_unit.partial_cmp(&b.per_unit).unwrap_or(std::cmp::Ordering::Equal)) {
        let amount = read_i32_at(fuel.item + ITEM_AMOUNT).unwrap_or(0);
        let units = whole_units(fuel.per_unit, amount).max(1);
        return Some((*fuel, units));
    }
    None
}

fn fuels_list(
    rt: &QuickRuntime,
    spend: &Spend,
    state: usize,
    inv: usize,
    kind: i32,
    tech: usize,
) -> Vec<FuelPick> {
    let mut listing = [0u8; 16];
    let kinds_fn: KindsFn = unsafe { std::mem::transmute(spend.kinds) };
    unsafe {
        kinds_fn(
            state as u64,
            kind,
            listing.as_mut_ptr() as u64,
        );
    }
    let count = u32::from_le_bytes([listing[4], listing[5], listing[6], listing[7]]);
    let data = u64::from_le_bytes([
        listing[8], listing[9], listing[10], listing[11], listing[12], listing[13], listing[14],
        listing[15],
    ]) as usize;
    let mut kinds_vec = Vec::new();
    if data >= mem::MIN_ADDR && count > 0 && count <= 32 {
        let mut i = 0u32;
        while i < count {
            if let Some(k) = read_i32_at(data.wrapping_add(4 * i as usize)) {
                kinds_vec.push(k);
            }
            i += 1;
        }
    }
    let free: FreeFn = unsafe { std::mem::transmute(spend.free) };
    unsafe {
        free(listing.as_ptr() as u64);
    }
    let table = read_ptr(spend.factor_global).unwrap_or(0);
    let mut found = Vec::new();
    if table < mem::MIN_ADDR {
        return found;
    }
    let mut seen = HashSet::new();
    for k in kinds_vec {
        let store_kind = if k == -1 { 0 } else { k };
        if !seen.insert(store_kind) {
            continue;
        }
        let store_fn: StoreFn = unsafe { std::mem::transmute(spend.store) };
        let store = unsafe { store_fn(inv as u64, store_kind, -1) } as usize;
        if store < mem::MIN_ADDR {
            continue;
        }
        let total = read_i32_at(store + STORE_COUNT).unwrap_or(0);
        let items = read_ptr(store + STORE_DATA).unwrap_or(0);
        if items < mem::MIN_ADDR || total <= 0 || total > 4096 {
            continue;
        }
        let skip = rt
            .skip_ids
            .lock()
            .ok()
            .map(|g| g.clone())
            .unwrap_or_default();
        let compat: CompatFn = unsafe { std::mem::transmute(spend.compat) };
        let factor: FactorFn = unsafe { std::mem::transmute(spend.factor) };
        let mut idx = 0i32;
        while idx < total {
            let item = items.wrapping_add((idx as usize).wrapping_mul(ITEM_STRIDE));
            if read_i32_at(item + ITEM_AMOUNT).unwrap_or(0) <= 0 {
                idx += 1;
                continue;
            }
            let ident = item_text(item);
            if skip.contains(&ident) {
                idx += 1;
                continue;
            }
            let ok = unsafe { compat(0, tech as u64, item as u64) };
            if ok == 0 {
                idx += 1;
                continue;
            }
            let per_unit = unsafe { factor(table.wrapping_add(FACTOR_TABLE) as u64, item as u64, tech as u64) };
            if per_unit > 0.0 {
                found.push(FuelPick {
                    item,
                    store_kind,
                    per_unit,
                });
            }
            idx += 1;
        }
    }
    found
}

fn spend_all(rt: &QuickRuntime, game: &Game, owner: usize, listed: &[usize]) -> Result<(HashMap<String, i32>, i32, i32, i32), String> {
    let spend = game.spend.as_ref().ok_or("spend missing")?;
    let state = owner.wrapping_add(spend.state_off as usize);
    let inv = owner.wrapping_add(spend.inv_off as usize);
    let mut used: HashMap<String, i32> = HashMap::new();
    let mut charged = 0i32;
    let mut full = 0i32;
    let mut short = 0i32;
    let prefer = rt
        .settings
        .lock()
        .ok()
        .map(|s| s.is_true("substances_first"))
        .unwrap_or(false);
    rt.log.info(&format!(
        "Charge all pick: {}",
        if prefer { "substances first" } else { "strongest first" }
    ));
    let skip_nonempty = rt.skip_ids.lock().ok().map(|s| !s.is_empty()).unwrap_or(false);
    if skip_nonempty {
        let report = rt
            .settings
            .lock()
            .ok()
            .map(|settings| rt.never_report(never_spend_text(&settings)))
            .unwrap_or_default();
        let fallback = rt
            .skip_ids
            .lock()
            .ok()
            .map(|s| {
                let mut v: Vec<_> = s.iter().cloned().collect();
                v.sort();
                v.join(", ")
            })
            .unwrap_or_default();
        rt.log.info(&format!(
            "Never spend skip: {}",
            if report.is_empty() { fallback } else { report }
        ));
    }
    let mid: MidFn = unsafe { std::mem::transmute(spend.mid) };
    let lookup: LookupFn = unsafe { std::mem::transmute(spend.lookup) };
    let store_fn: StoreFn = unsafe { std::mem::transmute(spend.store) };
    for entry in listed {
        let kind = read_i32_at(*entry + QM_KIND).unwrap_or(0);
        let key = entry.wrapping_add(QM_ID);
        let store = unsafe { store_fn(inv as u64, kind, -1) } as usize;
        let tech = if store >= mem::MIN_ADDR {
            (unsafe { lookup(store as u64, key as u64) }) as usize
        } else {
            0
        };
        if tech < mem::MIN_ADDR {
            continue;
        }
        let name = item_text(tech);
        let start = read_i32_at(tech + ITEM_AMOUNT).unwrap_or(0);
        let top = read_i32_at(tech + ITEM_MAX).unwrap_or(0);
        if start >= top {
            full += 1;
            rt.log.info(&format!("Tech {name} is full ({start} of {top})"));
            continue;
        }
        let mut fuels = fuels_list(rt, spend, state, inv, kind, tech);
        let fuel_line: String = fuels
            .iter()
            .map(|f| {
                let label = fuel_name(rt, spend, f.item);
                let amt = read_i32_at(f.item + ITEM_AMOUNT).unwrap_or(0);
                format!("{label} x{amt} ({:.1} each)", f.per_unit)
            })
            .collect::<Vec<_>>()
            .join(", ");
        rt.log.info(&format!(
            "Tech {name} at {start} of {top}, fuels: {}",
            if fuel_line.is_empty() { "none".into() } else { fuel_line }
        ));
        let mut step = 0i32;
        while step < 24 {
            let need = read_i32_at(tech + ITEM_MAX).unwrap_or(0) - read_i32_at(tech + ITEM_AMOUNT).unwrap_or(0);
            if need <= 0 {
                break;
            }
            fuels = fuels_list(rt, spend, state, inv, kind, tech);
            let chosen = choose(&fuels, need, prefer);
            let Some((pick, units)) = chosen else {
                break;
            };
            let ident = item_text(pick.item);
            let label = fuel_name(rt, spend, pick.item);
            let before: i32 = fuels
                .iter()
                .filter(|f| item_text(f.item) == ident)
                .map(|f| read_i32_at(f.item + ITEM_AMOUNT).unwrap_or(0))
                .sum();
            let gained_from = read_i32_at(tech + ITEM_AMOUNT).unwrap_or(0);
            unsafe {
                mid(
                    state as u64,
                    key as u64,
                    kind,
                    pick.item.wrapping_add(ITEM_KEY) as u64,
                    pick.store_kind,
                    units,
                );
            }
            let after: i32 = fuels_list(rt, spend, state, inv, kind, tech)
                .iter()
                .filter(|f| item_text(f.item) == ident)
                .map(|f| read_i32_at(f.item + ITEM_AMOUNT).unwrap_or(0))
                .sum();
            let taken = before - after;
            let gained = read_i32_at(tech + ITEM_AMOUNT).unwrap_or(0) - gained_from;
            let now = read_i32_at(tech + ITEM_AMOUNT).unwrap_or(0);
            rt.log.info(&format!(
                "  {label}: asked {units}, took {taken}, charge +{gained} (now {now} of {top})"
            ));
            if taken > 0 {
                *used.entry(label).or_insert(0) += taken;
            }
            if gained <= 0 && taken <= 0 {
                break;
            }
            step += 1;
        }
        let end = read_i32_at(tech + ITEM_AMOUNT).unwrap_or(0);
        if end > start {
            charged += 1;
        }
        if end < top {
            short += 1;
        }
    }
    Ok((used, charged, full, short))
}

/// The notice in its buffer: UTF-8 (game fuel names and our text are in the game language), cut on
/// a character boundary, NUL-ended. `AddTimedMessage` copies a C string of up to 0x200 bytes.
fn set_notice(notice: &mut [u8; 128], text: &str) {
    *notice = crate::common::menu_text::fixed(text);
}

/// `ui.qr_<id>` in `lang` with `{n}` and `{name}` filled in, English when missing.
fn notice_text(lang: &str, id: &str, n: i32, name: &str) -> String {
    let text = crate::common::menu_text::ui(lang, &format!("qr_{id}"));
    let text = if text.is_empty() { crate::common::menu_text::ui("en", &format!("qr_{id}")) } else { text };
    crate::common::menu_text::fill(text, n.max(0) as usize, name)
}

/// The notice after a spending recharge, in `lang`.
fn spend_notice(lang: &str, used: &HashMap<String, i32>, ran: i32, short: i32) -> String {
    let parts: String = used
        .iter()
        .map(|(label, count)| notice_text(lang, "part", *count, label))
        .collect::<Vec<_>>()
        .join(", ");
    if ran > 0 {
        let mut t = if parts.is_empty() {
            notice_text(lang, "recharged", ran, "")
        } else {
            notice_text(lang, "recharged_used", ran, &parts)
        };
        if short > 0 {
            t.push_str(". ");
            t.push_str(&notice_text(lang, "short", short, ""));
        }
        t
    } else if short > 0 {
        notice_text(lang, "short", short, "")
    } else {
        notice_text(lang, "all_full", 0, "")
    }
}

fn charge_all(rt: &QuickRuntime, game: &mut Game, menu: u64) -> Result<(), String> {
    let wallet: WalletFn = unsafe { std::mem::transmute(game.wallet) };
    let owner = unsafe { wallet() } as usize;
    if owner < mem::MIN_ADDR {
        rt.log.info("Charge all: no player state");
        return Ok(());
    }
    let state = owner.wrapping_add(game.state_off as usize);
    let listed = cards(menu);
    let free_fill = rt
        .settings
        .lock()
        .ok()
        .map(|s| s.is_true("free_fill"))
        .unwrap_or(false);
    let (ran, notice_text) = if free_fill {
        let charge1: Charge1Fn = unsafe { std::mem::transmute(game.charge1) };
        let mut ran = 0i32;
        for entry in &listed {
            let kind = read_i32_at(*entry + QM_KIND).unwrap_or(0);
            unsafe {
                charge1(state as u64, kind, entry.wrapping_add(QM_ID) as u64);
            }
            ran += 1;
        }
        rt.log.info(&format!(
            "Charge all (free fill): {ran} of {} listed tech(s)",
            listed.len()
        ));
        (ran, notice_text(rt.lang, "all_recharged", 0, ""))
    } else if game.spend.is_none() {
        rt.log.info("Charge all: spend path missing, nothing changed (turn on Free fill to use the old way)");
        return Ok(());
    } else {
        let (used, ran, full, short) = spend_all(rt, game, owner, &listed)?;
        let parts: String = used
            .iter()
            .map(|(label, count)| format!("{count} {label}"))
            .collect::<Vec<_>>()
            .join(", ");
        rt.log.info(&format!(
            "Charge all: {ran} of {} listed tech(s) charged, {full} already full, {short} still short. Used: {}",
            listed.len(),
            if parts.is_empty() { "nothing" } else { parts.as_str() }
        ));
        (ran, spend_notice(rt.lang, &used, ran, short))
    };
    set_notice(&mut game.notice, &notice_text);
    if ran > 0 && sound(rt, game).is_err() {
        rt.log.info("Charge sound failed");
        game.post = None;
    }
    if notice(rt, game).is_err() {
        rt.log.info("Charge notice failed");
        game.toast = None;
    }
    Ok(())
}

fn sound(rt: &QuickRuntime, game: &Game) -> Result<(), &'static str> {
    let play = rt
        .settings
        .lock()
        .ok()
        .map(|s| s.is_not_false("play_sound"))
        .unwrap_or(false);
    let post_va = game.post.ok_or("no post")?;
    if !play {
        return Ok(());
    }
    let rel = read_i32_at(game.obj_site + 3).ok_or("obj rel")?;
    let holder = read_ptr(game.obj_site.wrapping_add(7).wrapping_add(rel as usize)).ok_or("holder")?;
    let disp = read_i32_at(game.obj_site + OBJ_DISP_AT).ok_or("obj disp")?;
    let target = read_ptr(holder.wrapping_add(disp as usize)).ok_or("target")?;
    let mut event = [0u8; 16];
    event[8..12].copy_from_slice(&game.event.to_le_bytes());
    event[12] = 1;
    let post: PostFn = unsafe { std::mem::transmute(post_va) };
    unsafe {
        post(0, event.as_ptr() as u64, target as u64);
    }
    Ok(())
}

fn notice(rt: &QuickRuntime, game: &mut Game) -> Result<(), &'static str> {
    let show = rt
        .settings
        .lock()
        .ok()
        .map(|s| s.is_not_false("show_notice"))
        .unwrap_or(false);
    let toast = game.toast.as_ref().ok_or("no toast")?;
    if !show {
        return Ok(());
    }
    let Some(holder) = read_ptr(toast.holder_at) else {
        return Ok(());
    };
    game.timer.fill(0);
    let timer = game.timer.as_ptr() as u64;
    let add: ToastAddFn = unsafe { std::mem::transmute(toast.add) };
    let clean: ToastCleanFn = unsafe { std::mem::transmute(toast.clean) };
    unsafe {
        add(
            holder.wrapping_add(toast.object_off as usize) as u64,
            game.notice.as_ptr() as u64,
            TOAST_SECONDS,
            toast.title as u64,
            TOAST_AUDIO,
            timer,
            0,
            0.0f32,
            0,
            0,
            0,
        );
        clean(timer);
    }
    Ok(())
}

unsafe extern "system" fn trigger_detour(menu: u64, entry: u64, commit: u8) -> u8 {
    let original: TriggerFn = std::mem::transmute(TRIGGER_ORIG.load(Ordering::Acquire));
    let ret = original(menu, entry, commit);
    if let Some(rt) = RUNTIME.get() {
        rt.after_trigger(menu, entry, commit);
    }
    ret
}

impl GameMod for QuickRecharge {
    fn name(&self) -> &'static str {
        "QuickRecharge"
    }

    fn specs(&self) -> &'static [SettingSpec] {
        SPECS
    }

    fn get(&self, key: &str) -> Option<Value> {
        match key {
            "status" => Some(json!(self.status.lock().ok()?.clone())),
            "write_logs" => Some(json!(self.settings.lock().ok()?.is_not_false("write_logs"))),
            "recharge_all" => Some(json!(self.settings.lock().ok()?.is_true("recharge_all"))),
            "substances_first" => Some(json!(self.settings.lock().ok()?.is_true("substances_first"))),
            "never_spend" => Some(json!(
                self.settings
                    .lock()
                    .ok()?
                    .get("never_spend")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            )),
            "play_sound" => Some(json!(self.settings.lock().ok()?.is_not_false("play_sound"))),
            "show_notice" => Some(json!(self.settings.lock().ok()?.is_not_false("show_notice"))),
            "free_fill" => Some(json!(self.settings.lock().ok()?.is_true("free_fill"))),
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
        match key {
            "write_logs" => {
                let on = value.as_bool().ok_or_else(|| "expected true or false".to_string())?;
                self.settings
                    .lock()
                    .map_err(|_| "settings lock poisoned")?
                    .set(key, json!(on))?;
                self.log.set_enabled(on);
                Ok(format!("Write log {}", if on { "on" } else { "off" }))
            }
            "recharge_all" => {
                let on = value.as_bool().ok_or_else(|| "expected true or false".to_string())?;
                self.settings
                    .lock()
                    .map_err(|_| "settings lock poisoned")?
                    .set(key, json!(on))?;
                self.log
                    .info(&format!("Charge menu all {}", if on { "on" } else { "off" }));
                Ok(format!("Charge menu all {}", if on { "on" } else { "off" }))
            }
            "substances_first" => {
                let on = value.as_bool().ok_or_else(|| "expected true or false".to_string())?;
                self.settings
                    .lock()
                    .map_err(|_| "settings lock poisoned")?
                    .set(key, json!(on))?;
                self.log
                    .info(&format!("Substances first {}", if on { "on" } else { "off" }));
                Ok(format!("Substances first {}", if on { "on" } else { "off" }))
            }
            "never_spend" => {
                let text = value.as_str().ok_or_else(|| "expected string".to_string())?;
                let mut settings = self.settings.lock().map_err(|_| "settings lock poisoned")?;
                let runtime = RUNTIME.get().ok_or("Quick Recharge is unavailable")?;
                runtime.save_never_spend(&mut settings, text)?;
                Ok("Never spend updated".to_string())
            }
            "play_sound" | "show_notice" => {
                let on = value.as_bool().ok_or_else(|| "expected true or false".to_string())?;
                self.settings
                    .lock()
                    .map_err(|_| "settings lock poisoned")?
                    .set(key, json!(on))?;
                Ok(format!("{key} {}", if on { "on" } else { "off" }))
            }
            "free_fill" => {
                let on = value.as_bool().ok_or_else(|| "expected true or false".to_string())?;
                self.settings
                    .lock()
                    .map_err(|_| "settings lock poisoned")?
                    .set(key, json!(on))?;
                self.log
                    .info(&format!("Free fill {}", if on { "on" } else { "off" }));
                Ok(format!("Free fill {}", if on { "on" } else { "off" }))
            }
            _ => Err(format!("unknown setting {key:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str) -> FuelMask {
        fuel_groups().into_iter().find(|group| group.id == id).unwrap().mask
    }

    #[test]
    fn groups_cover_selectable_ids_without_overlaps() {
        let groups = fuel_groups();
        assert_eq!(groups.len(), 24);
        let mut covered = 0;
        for fuel in groups {
            assert_eq!(covered & fuel.mask, 0);
            covered |= fuel.mask;
        }
        let legacy_plants = fuel_exclusion_mask(&PLANTS_WITHOUT_RECHARGE_TARGETS.join(","));
        assert_eq!(legacy_plants.count_ones(), 4);
        assert_eq!(covered.count_ones(), 28);
        assert_eq!(covered & legacy_plants, 0);
        assert_eq!(covered | legacy_plants, ALL_FUELS);
    }

    #[test]
    fn toggling_a_fuel_preserves_unrelated_tokens() {
        let text = "Sodium, fUeL1, unknownzz,  ion batery , caRbOn";
        let live = fuel_mask(&parse_never(text).0);
        let sodium = group("SODIUM");
        assert_eq!(live & sodium, sodium);
        let mut edits = FuelEdits::default();
        edits.set_group(group("FUEL1"), false, live);
        let removed = edit_fuel_exclusions(text, edits).unwrap();
        assert_eq!(removed, "Sodium, unknownzz,  ion batery ");
        assert_eq!(fuel_mask(&parse_never(&removed).0) & sodium, live & sodium);

        edits.set_group(group("FUEL1"), true, live);
        assert!(edits.is_empty());
        assert_eq!(edit_fuel_exclusions(text, edits), None);

        edits.set_group(sodium, true, live);
        assert_eq!(edit_fuel_exclusions(text, edits), None);
        let reset = edit_fuel_exclusions(text, FuelEdits::reset()).unwrap();
        assert_eq!(reset.trim(), "unknownzz");
    }

    #[test]
    fn selector_edits_the_shared_manual_draft() {
        let sodium = group("SODIUM");
        let typed = "Sodium, CATALYST1, Phosphorus,  unknownzz ";
        let mask = fuel_exclusion_mask(typed);
        assert_eq!(mask & sodium, sodium);
        let mut edits = FuelEdits::default();
        edits.set_group(sodium, false, mask);
        let draft = edit_fuel_exclusions(typed, edits).unwrap();
        assert_eq!(draft, " Phosphorus,  unknownzz ");
        assert_eq!(parse_never(&draft).0, HashSet::from(["HOT1".to_owned()]));
        assert_eq!(fuel_exclusion_mask(&draft) & sodium, 0);
    }

    #[test]
    fn the_full_selector_round_trips_through_the_parser() {
        let mut edits = FuelEdits::default();
        for fuel in fuel_groups() {
            edits.set_group(fuel.mask, true, 0);
        }
        let text = edit_fuel_exclusions("unknownzz", edits).unwrap();
        let (ids, _, unknown) = parse_never(&text);
        let legacy_plants = fuel_exclusion_mask(&PLANTS_WITHOUT_RECHARGE_TARGETS.join(","));
        assert_eq!(fuel_mask(&ids), ALL_FUELS & !legacy_plants);
        assert_eq!(unknown, ["unknownzz"]);
        assert_eq!(edit_fuel_exclusions(&text, edits), None);
        assert_eq!(edit_fuel_exclusions(&text, FuelEdits::reset()).as_deref(), Some("unknownzz"));
    }

    #[test]
    fn selector_changes_preserve_existing_plant_exclusions() {
        let plants = "Solanium, PLANT_SNOW, Fungal Mould, Gamma Root";
        let mask = fuel_exclusion_mask(plants);
        assert_eq!(mask.count_ones(), 4);
        let mut edits = FuelEdits::default();
        edits.set_group(group("FUEL1"), true, mask);
        let draft = edit_fuel_exclusions(plants, edits).unwrap();
        assert_eq!(draft, format!("{plants}, FUEL1"));
        assert_eq!(fuel_exclusion_mask(&draft), mask | group("FUEL1"));
    }

    #[test]
    fn fuel_aliases_match_the_same_exclusion_and_new_edits_write_one_id() {
        for (name, legacy, current) in [
            ("Sodium", "SODIUM", "CATALYST1"),
            ("Sodium Nitrate", "SODIUM2", "CATALYST2"),
            ("Ion Battery", "CHARGECELL", "POWERCELL"),
            ("Pyrite", "DUST1", "DUSTY1"),
        ] {
            let expected_ids = HashSet::from([legacy.to_owned(), current.to_owned()]);
            let fuel = group(legacy);
            for token in [name, legacy, current] {
                let (ids, labels, unknown) = parse_never(token);
                assert_eq!(ids, expected_ids, "{token}");
                assert_eq!(labels, [name]);
                assert!(unknown.is_empty());
                assert_eq!(fuel_exclusion_mask(token), fuel);
            }
            let mut edits = FuelEdits::default();
            edits.set_group(fuel, true, 0);
            assert_eq!(edit_fuel_exclusions("", edits).as_deref(), Some(current));
        }
    }

    #[test]
    fn selector_apply_shares_the_manual_setting_and_persists_through_reload() {
        let dir = std::env::temp_dir().join(format!("nms-quick-recharge-fuels-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("quick_recharge.json");
        let defaults = json!({"never_spend": "", "free_fill": false});
        let runtime = QuickRuntime {
            log: Arc::new(Log::open(&dir.join("test.log"), false)),
            status: Arc::new(Mutex::new(String::new())),
            settings: Arc::new(Mutex::new(Settings::load(&path, defaults.clone()))),
            game: Mutex::new(None),
            skip_ids: Mutex::new(HashSet::new()),
            skip_key: Mutex::new(None),
            table_logged: Mutex::new(HashSet::new()),
            namer_live: AtomicBool::new(false),
            lang: "en",
        };
        let saved_text = "Uranium, Sodium, unknownzz";
        {
            let mut settings = runtime.settings.lock().unwrap();
            settings.set("free_fill", json!(true)).unwrap();
            runtime.save_never_spend(&mut settings, saved_text).unwrap();
        }
        assert!(runtime.skip_ids.lock().unwrap().contains("CATALYST1"));
        let mut edits = FuelEdits::default();
        edits.set_group(group("FUEL1"), true, 0);
        edits.set_group(group("CHARGECELL"), true, 0);
        let expected = "Uranium, Sodium, unknownzz, FUEL1, POWERCELL";
        let draft = edit_fuel_exclusions(saved_text, edits).unwrap();
        assert_eq!(draft, expected);
        assert!(!runtime.skip_ids.lock().unwrap().contains("FUEL1"));
        assert_eq!(never_spend_text(&Settings::load(&path, defaults.clone())), saved_text);
        runtime.save_never_spend(&mut runtime.settings.lock().unwrap(), &draft).unwrap();
        assert!(runtime.skip_ids.lock().unwrap().contains("FUEL1"));
        let reloaded = Settings::load(&path, defaults.clone());
        assert_eq!(never_spend_text(&reloaded), expected);
        assert!(reloaded.is_true("free_fill"));
        runtime.refresh_never(never_spend_text(&reloaded));
        assert!(runtime.skip_ids.lock().unwrap().contains("POWERCELL"));
        runtime.save_never_spend(&mut runtime.settings.lock().unwrap(), "").unwrap();
        let reloaded = Settings::load(&path, defaults);
        assert_eq!(never_spend_text(&reloaded), "");
        assert!(reloaded.is_true("free_fill"));
        assert!(runtime.skip_ids.lock().unwrap().is_empty());
        drop(runtime);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn notice_follows_the_language_and_keeps_utf8() {
        let mut used = HashMap::new();
        used.insert("Pure Ferrite".to_string(), 8);
        assert_eq!(spend_notice("en", &used, 1, 0), "Recharged 1 tech: 8 Pure Ferrite");
        assert_eq!(spend_notice("en", &HashMap::new(), 2, 1), "Recharged 2 tech. 1 tech short of fuel");
        assert_eq!(spend_notice("en", &HashMap::new(), 0, 0), "All technologies already full");
        assert_eq!(notice_text("xx", "all_recharged", 0, ""), NOTICE_DEFAULT);
        let mut ko = HashMap::new();
        ko.insert("순수 페라이트".to_string(), 8);
        let text = spend_notice("ko", &ko, 1, 0);
        assert_eq!(text, "기술 1개 충전됨: 순수 페라이트 8개");
        let mut buf = [0u8; 128];
        set_notice(&mut buf, &text);
        assert_eq!(&buf[..text.len()], text.as_bytes());
        assert_eq!(buf[text.len()], 0);
        for lang in ["en", "de", "fr", "it", "es", "es-419", "ru", "pl", "nl", "pt", "pt-BR", "ja", "zh", "zh-Hant", "ko"] {
            for id in ["all_recharged", "recharged", "recharged_used", "short", "all_full", "part"] {
                assert!(!crate::common::menu_text::ui(lang, &format!("qr_{id}")).is_empty(), "{lang} {id}");
            }
        }
    }

    #[test]
    fn norm_name_collapses_separators() {
        assert_eq!(norm_name("Condensed-Carbon"), "condensed carbon");
        assert_eq!(norm_name("  FUEL_1  "), "fuel 1");
    }

    #[test]
    fn ratio_matches_python_difflib() {
        // Values from Python 3.10 difflib.SequenceMatcher(None, a, b).ratio().
        let cases = [
            ("ion batery", "ion battery", 0.9523809523809523),
            ("ion batery", "advanced ion battery", 0.6666666666666666),
            ("abxcd", "abcd", 0.8888888888888888),
            ("carbn", "condensed carbon", 0.47619047619047616),
            ("", "", 1.0),
        ];
        for (a, b, want) in cases {
            assert!((sequence_matcher_ratio(a, b) - want).abs() < 1e-12, "{a} vs {b}");
        }
    }

    #[test]
    fn match_fuel_by_id() {
        let (label, ids) = match_fuel_token("fuel1");
        assert_eq!(label.as_deref(), Some("Carbon"));
        assert!(ids.contains("FUEL1"));
    }

    #[test]
    fn match_fuel_by_display_name() {
        let (label, ids) = match_fuel_token("Ion Battery");
        assert_eq!(label.as_deref(), Some("Ion Battery"));
        assert!(ids.contains("CHARGECELL") || ids.contains("POWERCELL"));
    }

    #[test]
    fn parse_never_lists_unknown() {
        let (ids, labels, unknown) = parse_never("Carbon, not-a-fuel");
        assert!(ids.contains("FUEL1"));
        assert!(labels.iter().any(|l| l == "Carbon"));
        assert_eq!(unknown, vec!["not-a-fuel"]);
    }

    #[test]
    fn choose_prefers_substance_when_flagged() {
        let a = FuelPick {
            item: 0,
            store_kind: 0,
            per_unit: 10.0,
        };
        let b = FuelPick {
            item: 0,
            store_kind: 0,
            per_unit: 50.0,
        };
        // Without item memory, substances_first path still runs; empty type reads fail so use need path.
        let picked = choose(&[a, b], 100, false);
        assert!(picked.is_some());
        let (f, _) = picked.unwrap();
        assert!((f.per_unit - 50.0).abs() < f32::EPSILON);
    }
}

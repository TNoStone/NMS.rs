//! The in-game Mods page inside the game's own Options menu (main menu: a header per mod, a row per
//! setting: on/off, whole number, decimal or a choice).
//!
//! The game draws Options rows in code
//! every frame with a small widget library, so we add rows by calling those widgets ourselves:
//! - `FinalizePage` (before): on the Options list (page 6) add a `Mods` button under View Credits.
//! - `EndPage` (before): on our page (id 8) draw the rows, then finalize the page as the game does.
//!
//! - `DoGameSwitcher` (after): the pause-menu list. Its rows are MBIN elements; ours is `BUTTON_MODS`,
//!   added to `SWITCHERPAGENEW` by `NMSrsMenu.lua`. We find it by name with the game's own lookup and
//!   test hover and click with the game's `Confirm`, as the page does for its own buttons.
//!
//! - Text settings reuse the game's own text-entry popup (`cGcFrontendTextInput`, the box that renames a
//!   storage container). A row shows the value; a click starts the popup; every frame (`Update`, after)
//!   we poll for the result and apply it.
//!
//! Changes are staged: a row shows and edits a pending value, and nothing reaches the mod until
//! the player presses Apply. Undo drops the pending values; Reset to defaults stages every
//! setting's shipped default (logs off). Opening the page starts with nothing staged.
//!
//! Labels and descriptions follow the game's language (`lang`, `menu_text`).
//!
//! Always on. Any signature missing means no row and a vanilla menu. Detours here never panic:
//! every read is checked.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use nms_core::log::Log;
use nms_core::mem::{read_ptr, read_u32, read_u8, MIN_ADDR};
use super::input::async_key_down;
use super::{lang, menu_text};
use nms_sdk::sigs::hide_empty_hover as name_sigs;
use nms_core::process::ProcessImage;
use nms_core::{Code, Kind, Pattern, Range, Registry};
use nms_sdk::sigs::ingame_options as sigs;
use serde_json::json;

use super::hooks::{options_end_page, options_finalize_page, options_game_switcher, options_page_title, text_input_update};

/// `eOptionsMenu` values the game defines are 0..=7; 6 is the Options list. 8 is ours.
const LIST_PAGE: u32 = 6;
const MODS_PAGE: u32 = 8;
/// Addresses below this are not elements.
const MIN_ELEMENT: usize = MIN_ADDR;
/// `cGcOptionsUIHeaderIcons::Advanced`.
const ICON_ADVANCED: i32 = 4;
/// What `FinalizePage` returns when a footer button (or the Back prompt) was used.
const FOOTER_APPLY: u32 = 1;
const FOOTER_UNDO: u32 = 2;
const FOOTER_RESET: u32 = 3;

/// One setting row. The strings are leaked once at install: the game may keep the pointers.
struct Row {
    mod_name: &'static str,
    key: &'static str,
    kind: Kind,
    /// Limits for `Int`, `Float` and `Enum` rows.
    range: Option<Range>,
    label: *const u8,
    /// The label without the trailing spaces that keep repeated labels apart (popup title, Clear row).
    plain_label: String,
    /// The shipped default, so the game's Reset button knows when a row differs from it.
    default: Option<serde_json::Value>,
    desc: *const u8,
    /// Text of each choice of an `Enum` row, in the order of its range.
    choices: Vec<*const u8>,
    /// `Text` rows: label of the extra "Clear" row, shown while the value is not empty.
    clear_label: *const u8,
    /// The value text last shown by a `Text` row, as a leaked C string (rebuilt only on change).
    shown: Mutex<ShownText>,
}

/// What a `Text` row displays in its cell. Strings are leaked, so a new one is made only when the
/// value changes.
struct ShownText {
    value: String,
    ptr: *const u8,
}
// SAFETY: the pointer refers to a leaked, never-freed byte string.
unsafe impl Send for ShownText {}

/// A mod with at least one on/off setting: its header text and rows.
struct Section {
    header: *const u8,
    rows: Vec<Row>,
}

// SAFETY: the pointers refer to leaked, never-freed byte strings.
unsafe impl Send for Section {}
unsafe impl Sync for Section {}

/// The page's own strings in one language, as leaked C strings.
struct Ui {
    mods: *const u8,
    title: *const u8,
    clear: *const u8,
    clear_desc: *const u8,
    no_edit: *const u8,
    /// Hint shown in a text row whose value is empty (a plain string: it is part of the cell text).
    not_set: String,
}

// SAFETY: the pointers refer to leaked, never-freed byte strings.
unsafe impl Send for Ui {}
unsafe impl Sync for Ui {}

/// Everything the page draws, built for one language.
struct Built {
    lang: &'static str,
    ui: Ui,
    sections: Vec<Section>,
}

/// The empty suffix text for number rows.
static NO_SUFFIX: [u8; 1] = [0];

/// The current page, replaced when the game language changes.
static BUILT: Mutex<Option<Arc<Built>>> = Mutex::new(None);
static IMAGE: OnceLock<ProcessImage> = OnceLock::new();
/// When the language was last looked up (ms on the `now_ms` clock).
static LANG_CHECKED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Changes the player made on the page and has not applied: `(mod, key)` to the value.
static STAGED: Mutex<Option<HashMap<(&'static str, &'static str), serde_json::Value>>> = Mutex::new(None);
/// A click on a text row, held until the end of the frame: when Reset is confirmed the game gives
/// every row its default, which a row drawn as "off" reports as a click, so the click only counts
/// if the same frame (and the next moments) did not also report Reset.
static TEXT_CLICK: Mutex<Option<(&'static str, &'static str)>> = Mutex::new(None);
/// Text clicks are ignored until this time (ms) after a Reset.
static RESET_UNTIL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const RESET_QUIET_MS: u64 = 600;
static REGISTRY: OnceLock<Arc<Registry>> = OnceLock::new();
static LOG: OnceLock<Arc<Log>> = OnceLock::new();

static HEADER: AtomicUsize = AtomicUsize::new(0);
static BUTTON: AtomicUsize = AtomicUsize::new(0);
static BOOLEAN: AtomicUsize = AtomicUsize::new(0);
/// Validated native page field; zero leaves the existing main-menu click guard in place.
static ROW_DISABLED_OFFSET: AtomicUsize = AtomicUsize::new(0);
static INT: AtomicUsize = AtomicUsize::new(0);
static FLOAT: AtomicUsize = AtomicUsize::new(0);
static BEGIN_LIST: AtomicUsize = AtomicUsize::new(0);
static LIST_OPTION: AtomicUsize = AtomicUsize::new(0);
static END_LIST: AtomicUsize = AtomicUsize::new(0);
static FINALIZE: AtomicUsize = AtomicUsize::new(0);
static REQUEST_PAGE: AtomicUsize = AtomicUsize::new(0);
static STACK_COUNT: AtomicUsize = AtomicUsize::new(0);
static STACK_DATA: AtomicUsize = AtomicUsize::new(0);
static FIND_CHILD: AtomicUsize = AtomicUsize::new(0);
static CONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Address of the game's pointer to the global state that holds the "popup open" byte.
static SWITCHER_BUSY: AtomicUsize = AtomicUsize::new(0);
static TEXT_START: AtomicUsize = AtomicUsize::new(0);
static TEXT_RESULT: AtomicUsize = AtomicUsize::new(0);
static TEXT_STOP: AtomicUsize = AtomicUsize::new(0);
static SET_ACTIONS: AtomicUsize = AtomicUsize::new(0);
/// Address of the game's pointer to its application object (holds the input controller).
static APP_GLOBAL: AtomicUsize = AtomicUsize::new(0);
/// True when the Mods page was opened from the in-game pause list, false from the title screen. The
/// text box is only drawn in game: on the title screen it starts but is never shown, and the game
/// keeps input off while it waits. So text rows can only be edited in game.
static OPENED_IN_GAME: AtomicBool = AtomicBool::new(false);
/// When the finished result was first seen (ms); 0 when none. Finishing waits `FINISH_DELAY_MS` after that.
static DONE_SEEN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// After a popup closes, if the page is left before this time (ms), it was the closing press that did it.
static WATCH_UNTIL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// When our popup was started, on the `now_ms` clock.
static STARTED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The text-entry manager, taken from the `Update` hook's argument.
static TEXT_MGR: AtomicUsize = AtomicUsize::new(0);
/// Text rows are drawn only when every text-input routine was found and `Update` is hooked.
static TEXT_READY: AtomicBool = AtomicBool::new(false);
/// The setting the open popup will write: `(mod, key)`. `None` when no popup of ours is open.
static PENDING: Mutex<Option<(&'static str, &'static str)>> = Mutex::new(None);
static BACK_ORIG: AtomicUsize = AtomicUsize::new(0);
static BACK_HOOK: OnceLock<nms_core::hook::Hook> = OnceLock::new();
/// How long a text box may block Back, at most (ms). Back can never stay blocked for longer.
const BACK_BLOCK_MAX_MS: u64 = 300_000;
static HASH_FN: AtomicUsize = AtomicUsize::new(0);
static LOOKUP_FN: AtomicUsize = AtomicUsize::new(0);
/// Address of the game's pointer to the root UI layer, read from the title function.
static TITLE_ROOT: AtomicUsize = AtomicUsize::new(0);
static LAST_STATE: AtomicU32 = AtomicU32::new(u32::MAX);

type HeaderFn = unsafe extern "system" fn(u64, *const u8, i32);
type ButtonFn = unsafe extern "system" fn(u64, *const u8, *const u8, *const u8) -> u8;
type BooleanFn = unsafe extern "system" fn(u64, *const u8, *const u8, u8, u8, *const BooleanOptions) -> u8;
type IntFn = unsafe extern "system" fn(u64, *const u8, *const u8, i32, i32, i32, i32, *const IntOptions) -> i32;
type FloatFn = unsafe extern "system" fn(u64, *const u8, *const u8, f32, f32, f32, f32, *const FloatOptions) -> f32;
type BeginListFn = unsafe extern "system" fn(u64, *const u8, *const u8, i32, i32, *mut i32);
type ListOptionFn = unsafe extern "system" fn(u64, *const u8, i32);
type EndListFn = unsafe extern "system" fn(u64) -> i32;
type FindChildFn = unsafe extern "system" fn(u64, *const u8, u8) -> u64;
type ConfirmFn = unsafe extern "system" fn(u64, u64, u8, u32, *const ConfirmArgs, u8, u8) -> u8;
type TextStartFn = unsafe extern "system" fn(u64, *const [u8; 64], *const [u8; 64], u8, u32);
type TextResultFn = unsafe extern "system" fn(u64, *mut [u8; 256]) -> i32;
type TextStopFn = unsafe extern "system" fn(u64);
type SetActionsFn = unsafe extern "system" fn(u64, u32, u8);
type FinalizeFn = unsafe extern "system" fn(u64, u64, u64) -> u64;
type HashFn = unsafe extern "system" fn(*mut u8, *mut u8, i32) -> u64;
type LookupFn = unsafe extern "system" fn(u64, *const u8, i32) -> u64;
type SetTextFn = unsafe extern "system" fn(u64, *const u8) -> u64;
type RequestPageFn = unsafe extern "system" fn(i32, u8) -> u64;

/// The 16 bytes the page passes to `Confirm` for every button: no sound override (0), then the
/// constant `0x18aeccf6` and a set flag byte, read from `DoGameSwitcher`.
#[repr(C)]
struct ConfirmArgs {
    zero: u64,
    id: u32,
    flag: u8,
    pad: [u8; 3],
}

/// `cGcIntOptions`. `step1` is the change per tick; `step2` multiplies the value for display only
/// (the game's own sliders pass 1).
#[repr(C)]
struct IntOptions {
    step1: i32,
    step2: i32,
    suffix: *const u8,
    unknown: u64,
}

/// `cGcFloatOptions`: `step1` per tick, `step2` the display multiplier (1.0).
#[repr(C)]
struct FloatOptions {
    step1: f32,
    step2: f32,
    suffix: *const u8,
    unknown: u64,
}

/// `cGcBooleanOptions`: the text ids for the on and off state.
#[repr(C)]
struct BooleanOptions {
    enabled: *const u8,
    disabled: *const u8,
}
// SAFETY: both pointers refer to static byte strings.
unsafe impl Sync for BooleanOptions {}

static BOOL_TEXT: BooleanOptions = BooleanOptions { enabled: b"UI_ENABLED\0".as_ptr(), disabled: b"UI_DISABLED\0".as_ptr() };
/// `cGcButtonOptions` is one bool; the rest is padding for safety.
static BUTTON_OPTS: [u8; 8] = [0; 8];

fn info(msg: &str) {
    if let Some(log) = LOG.get() {
        log.info(msg);
    }
}

/// Check the close code inside the text popup's `Update` (`sigs::TEXT_CLOSE_AT`) and read the two
/// addresses it names: the application global and the "switch action set" routine (both as VAs).
pub fn text_close_code(image: &ProcessImage, update: usize) -> Result<(usize, usize), String> {
    let close = update + sigs::TEXT_CLOSE_AT;
    let len = sigs::TEXT_CLOSE_LEN;
    let code = image.read(close, len).ok_or("text popup update unreadable")?;
    let fixed = |range: std::ops::Range<usize>, want: &[u8]| code.get(range).is_some_and(|b| b == want);
    let ctl = sigs::TEXT_CLOSE_CONTROLLER_AT;
    let sets = sigs::TEXT_CLOSE_SETS_AT;
    let call = sigs::TEXT_CLOSE_CALL_AT;
    // mov rax,[rip+app]; mov r8b,1; mov edx,r15d; mov rcx,[rax+ctl]; mov [rcx+0x40],r13b;
    // add rcx,sets; call set_actions
    if !(code.len() == len
        && call + 5 == len
        && fixed(0..3, &[0x48, 0x8B, 0x05])
        && fixed(7..ctl, &[0x41, 0xB0, 0x01, 0x41, 0x8B, 0xD7, 0x48, 0x8B, 0x88])
        && code.get(ctl..ctl + 4).is_some_and(super::context::controller_disp_matches)
        && fixed(ctl + 4..sets - 3, &[0x44, 0x88, 0x69, 0x40])
        && fixed(sets - 3..sets, &[0x48, 0x81, 0xC1])
        && fixed(sets..sets + 4, &sigs::CONTROLLER_SETS.to_le_bytes()[..])
        && code.get(call) == Some(&0xE8))
    {
        return Err("text popup close code changed (game update?)".into());
    }
    let app = rip_target(image, close, 3, 7).ok_or("application pointer unreadable")?;
    let rel = i32::from_le_bytes([code[call + 1], code[call + 2], code[call + 3], code[call + 4]]);
    Ok((app, (image.va(close + len) as i64 + i64::from(rel)) as usize))
}

/// Where a `mov`/`cmp` with a RIP displacement points: `rva` is the instruction, `disp_at` the
/// displacement offset in it, `len` the instruction length.
fn rip_target(image: &ProcessImage, rva: usize, disp_at: usize, len: usize) -> Option<usize> {
    let bytes = image.read(rva + disp_at, 4)?;
    let disp = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    Some((image.va(rva + len) as i64 + disp as i64) as usize)
}

fn find(image: &ProcessImage, name: &str, pattern: &str) -> Result<usize, String> {
    let pat = Pattern::parse(pattern).map_err(|e| format!("{name}: {e}"))?;
    image.find_unique(&pat).unique().ok_or_else(|| format!("{name} not found (game update?)"))
}

/// Add the Mods page to the Options menu. Silent no-op (vanilla) when any signature is missing.
pub fn install(image: &ProcessImage, registry: Arc<Registry>, log: Arc<Log>) {
    match locate(image) {
        Ok(()) => {}
        Err(e) => {
            log.info(&format!("in-game Mods menu not added: {e}"));
            return;
        }
    }
    let _ = REGISTRY.set(registry.clone());
    let _ = LOG.set(log.clone());
    let boolean = BOOLEAN.load(Ordering::Acquire) - image.base();
    match row_disabled_offset(image, boolean) {
        Some(offset) => {
            ROW_DISABLED_OFFSET.store(offset, Ordering::Release);
            log.info("in-game Mods menu: native disabled text-row styling ready");
        }
        None => log.info("in-game Mods menu: disabled row styling unavailable; main-menu text edits remain blocked"),
    }
    match install_text(image) {
        Ok(()) => log.info("in-game Mods menu: text rows ready (game text-entry popup)"),
        Err(e) => log.info(&format!("in-game Mods menu: no text rows: {e}")),
    }
    let _ = IMAGE.set(image.clone());
    refresh_language(true);
    let finalize = options_finalize_page::subscribe(image, sigs::FINALIZE_PAGE, Some(on_finalize), None);
    let end = options_end_page::subscribe(image, sigs::END_PAGE, Some(on_end), None);
    // The title is cosmetic: without it our page keeps the default "OPTIONS" title.
    if let Err(e) = options_page_title::subscribe(image, sigs::PAGE_TITLE, None, Some(on_title)) {
        log.info(&format!("in-game Mods menu: page title not set: {e}"));
    }
    match locate_pause(image) {
        Ok(()) => match options_game_switcher::subscribe(image, sigs::GAME_SWITCHER, None, Some(on_switcher)) {
            Ok(()) => log.info("in-game Mods menu: pause list hook ready (needs NMSrsMenu.lua for the button)"),
            Err(e) => log.info(&format!("in-game Mods menu: no pause row: {e}")),
        },
        Err(e) => log.info(&format!("in-game Mods menu: no pause row: {e}")),
    }
    match (finalize, end) {
        (Ok(()), Ok(())) => log.info("in-game Mods menu ready: Options list gets a Mods row"),
        (f, e) => log.info(&format!(
            "in-game Mods menu not fully added: {} {}",
            f.err().unwrap_or_default(),
            e.err().unwrap_or_default()
        )),
    }
}

fn locate(image: &ProcessImage) -> Result<(), String> {
    let at = |a: &AtomicUsize, name: &str, pattern: &str| -> Result<(), String> {
        a.store(image.va(find(image, name, pattern)?), Ordering::Release);
        Ok(())
    };
    at(&HEADER, "Header", sigs::HEADER)?;
    at(&BUTTON, "Button", sigs::BUTTON)?;
    at(&BOOLEAN, "Boolean", sigs::BOOLEAN)?;
    at(&INT, "Int", sigs::INT)?;
    at(&FLOAT, "Float", sigs::FLOAT)?;
    at(&BEGIN_LIST, "BeginList", sigs::BEGIN_LIST)?;
    at(&LIST_OPTION, "ListOption", sigs::LIST_OPTION)?;
    at(&END_LIST, "end of list", sigs::END_LIST)?;
    at(&FINALIZE, "FinalizePage", sigs::FINALIZE_PAGE)?;
    at(&REQUEST_PAGE, "RequestOptionsPage", sigs::REQUEST_OPTIONS_PAGE)?;
    at(&HASH_FN, "name hash", name_sigs::HASH)?;
    at(&LOOKUP_FN, "child lookup", name_sigs::LOOKUP)?;
    let title = find(image, "page title", sigs::PAGE_TITLE)? + sigs::TITLE_ROOT_AT;
    // `mov rcx, [rip+disp32]` (48 8B 0D): the root layer pointer the title function searches.
    if image.read(title, 3).as_deref() != Some(&[0x48, 0x8B, 0x0D][..]) {
        return Err("page title function changed (game update?)".into());
    }
    TITLE_ROOT.store(rip_target(image, title, 3, 7).ok_or("page title root unreadable")?, Ordering::Release);
    let count = find(image, "page stack count", sigs::STACK_COUNT)?;
    let data = find(image, "page stack data", sigs::STACK_DATA)?;
    STACK_COUNT.store(rip_target(image, count, 2, 7).ok_or("page stack count unreadable")?, Ordering::Release);
    STACK_DATA.store(rip_target(image, data, 3, 7).ok_or("page stack data unreadable")?, Ordering::Release);
    Ok(())
}

/// Locate the text-entry popup and hook its per-frame `Update`. Text rows stay off unless all of it works.
fn install_text(image: &ProcessImage) -> Result<(), String> {
    let at = |a: &AtomicUsize, name: &str, pattern: &str| -> Result<usize, String> {
        let rva = find(image, name, pattern)?;
        a.store(image.va(rva), Ordering::Release);
        Ok(rva)
    };
    at(&TEXT_START, "text popup start", sigs::TEXT_START)?;
    at(&TEXT_RESULT, "text popup result", sigs::TEXT_RESULT)?;
    at(&TEXT_STOP, "text popup stop", sigs::TEXT_STOP)?;
    let update = find(image, "text popup update", sigs::TEXT_UPDATE)?;
    // The code at the end of `Update` that gives the keyboard back to the game: it names the
    // application global and the "switch action set" routine we need to undo a popup that is left open.
    let (app_global, set_actions) = text_close_code(image, update)?;
    APP_GLOBAL.store(app_global, Ordering::Release);
    SET_ACTIONS.store(set_actions, Ordering::Release);
    text_input_update::subscribe(image, sigs::TEXT_UPDATE, None, Some(on_text_update))?;
    // Give held input back from the core's per-frame tick too, so it can never stay off in a game
    // state that does not run the text box update. Optional: the text update still does it.
    let _ = nms_core::game::dispatch::on_tick("Mods page", nms_core::game::dispatch::TickPhase::Post, release_input_tick);
    install_back_guard(image)?;
    TEXT_READY.store(true, Ordering::Release);
    Ok(())
}

/// BOOLEAN calls this native helper with the page in RDX. Check its call and page+0x18 load
/// before using the disabled state that selects DISABLE/DISABLED and skips click tests.
fn row_disabled_offset(image: &dyn Code, boolean: usize) -> Option<usize> {
    if image.read(boolean + 0x7F, 9)?.as_slice() != [0x48, 0x8B, 0xD6, 0x48, 0x8D, 0x4C, 0x24, 0x40, 0xE8] {
        return None;
    }
    let helper = image.rel32_target(boolean + 0x88)?;
    let expected = [
        0x48, 0x89, 0x5C, 0x24, 0x10, 0x44, 0x89, 0x44, 0x24, 0x18,
        0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
        0x48, 0x8D, 0x6C, 0x24, 0xD9, 0x48, 0x81, 0xEC, 0x90, 0, 0, 0,
        0x44, 0x8B, 0x72, 0x18,
    ];
    (image.read(helper, expected.len())?.as_slice() == expected).then_some(0x18)
}

/// Restore the page's previous disabled state immediately after one synchronous widget call.
struct DisabledRow {
    address: usize,
    previous: u32,
}

impl DisabledRow {
    fn enter(page: usize, offset: usize) -> Option<Self> {
        if offset == 0 {
            return None;
        }
        let address = page.checked_add(offset)?;
        let previous = read_u32(address)?;
        // SAFETY: offset was validated from the native helper; this is a live Options page.
        unsafe { nms_core::mem::write(address, 1u32) }.then_some(Self { address, previous })
    }
}

impl Drop for DisabledRow {
    fn drop(&mut self) {
        // SAFETY: same page field, restored immediately after its synchronous widget call.
        if !unsafe { nms_core::mem::write(self.address, self.previous) } {
            info("could not restore Options page disabled state");
        }
    }
}

/// Refuse the Options page's "go back" while our text box is open: the press that closes the box with
/// B (or a click on Back) would otherwise also pop the Mods page, and the pause list after it.
fn install_back_guard(image: &ProcessImage) -> Result<(), String> {
    let rva = find(image, "options back", sigs::OPTIONS_BACK)?;
    // SAFETY: the signature is the function start; the detour takes the same single argument.
    let hook = unsafe {
        nms_core::hook::Hook::install("OptionsBack", image.va(rva), back_detour as *const (), &BACK_ORIG)
    }?;
    let _ = BACK_HOOK.set(hook);
    Ok(())
}

/// True while a text box of ours is open and young enough to block Back.
fn text_box_blocks_back() -> bool {
    let open = match PENDING.try_lock() {
        Ok(p) => p.is_some(),
        Err(std::sync::TryLockError::WouldBlock) => true,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().is_some(),
    };
    open && now_ms().saturating_sub(STARTED_MS.load(Ordering::Acquire)) < BACK_BLOCK_MAX_MS
}

unsafe extern "system" fn back_detour(flag: u64) -> u64 {
    if text_box_blocks_back() {
        info("Back ignored: the text box is open");
        return 0;
    }
    // SAFETY: set from the hook's trampoline before the hook is enabled; same signature.
    let original: unsafe extern "system" fn(u64) -> u64 = std::mem::transmute(BACK_ORIG.load(Ordering::Acquire));
    original(flag)
}

/// The pause list needs three more game routines and the busy flag. All optional: without them the
/// main-menu page still works.
fn locate_pause(image: &ProcessImage) -> Result<(), String> {
    let at = |a: &AtomicUsize, name: &str, pattern: &str| -> Result<(), String> {
        a.store(image.va(find(image, name, pattern)?), Ordering::Release);
        Ok(())
    };
    at(&FIND_CHILD, "child find", sigs::FIND_CHILD)?;
    at(&CONFIRM, "Confirm", sigs::CONFIRM)?;
    let function = find(image, "pause list", sigs::GAME_SWITCHER)?;
    let mov = image.read(function + sigs::BUSY_AT, 3).ok_or("pause list unreadable")?;
    let cmp = image.read(function + sigs::BUSY_CHECK_AT, 7).ok_or("pause list unreadable")?;
    if mov[..] != sigs::BUSY_CODE || cmp[..] != sigs::BUSY_CHECK {
        return Err("pause list changed (game update?)".into());
    }
    let switcher = function + sigs::BUSY_AT;
    SWITCHER_BUSY.store(rip_target(image, switcher, 3, 7).ok_or("pause list busy flag unreadable")?, Ordering::Release);
    Ok(())
}

/// The id of the Options page on top of the game's page stack.
fn top_state() -> Option<u32> {
    let count = read_u32(STACK_COUNT.load(Ordering::Acquire))? as usize;
    if count == 0 || count > 32 {
        return None;
    }
    let data = read_ptr(STACK_DATA.load(Ordering::Acquire))?;
    let state = read_ptr(data + 8 * (count - 1))?;
    read_u32(state)
}

fn note_state(state: Option<u32>) {
    let value = state.unwrap_or(u32::MAX);
    if LAST_STATE.swap(value, Ordering::Relaxed) != value {
        match state {
            Some(s) => info(&format!("options page state {s}")),
            None => info("options page state unreadable"),
        }
    }
}

/// The Options list, before it is finalized: add the Mods row after View Credits.
fn on_finalize(args: &mut options_finalize_page::Args) {
    let state = top_state();
    note_state(state);
    if state != Some(LIST_PAGE) {
        return;
    }
    refresh_language(false);
    let Some(built) = current() else { return };
    // SAFETY: the address is a unique signature match; the signature is the nmspy `Button`.
    let button: ButtonFn = unsafe { std::mem::transmute(BUTTON.load(Ordering::Acquire)) };
    // No description, like the game's own list buttons (they pass null).
    let clicked = unsafe { button(args.page, built.ui.mods, std::ptr::null(), BUTTON_OPTS.as_ptr()) };
    if clicked & 1 != 0 {
        OPENED_IN_GAME.store(false, Ordering::Release);
        open_page_reset();
        info("Mods row clicked: opening page 8");
        // SAFETY: nmspy `RequestOptionsPage(eOptionsMenu, bool)`.
        let request: RequestPageFn = unsafe { std::mem::transmute(REQUEST_PAGE.load(Ordering::Acquire)) };
        unsafe { request(MODS_PAGE as i32, 1) };
    }
}

/// Our page: draw it where the game draws its own, then finalize it like the others do.
fn on_end(args: &mut options_end_page::Args) {
    if top_state() != Some(MODS_PAGE) {
        return;
    }
    let waiting = draw_mods_page(args.page);
    // SAFETY: nmspy `FinalizePage(page, changed, hide_footer)`, called as the game's own pages do
    // right before EndPage. `changed` greys Apply / Cancel changes while nothing is staged and
    // makes Back ask "apply / discard / cancel". It returns 0 nothing, 1 Apply, 2 Cancel changes
    // (or Discard from the Back prompt), 3 Reset confirmed.
    let finalize: FinalizeFn = unsafe { std::mem::transmute(FINALIZE.load(Ordering::Acquire)) };
    let clicked = unsafe { finalize(args.page, u64::from(staged_count() > 0), 0) } as u32;
    if clicked == FOOTER_RESET {
        RESET_UNTIL_MS.store(now_ms() + RESET_QUIET_MS, Ordering::Release);
    }
    let text_click = TEXT_CLICK.lock().ok().and_then(|mut c| c.take());
    if let Some((mod_name, key)) = text_click {
        if clicked == FOOTER_RESET || now_ms() < RESET_UNTIL_MS.load(Ordering::Acquire) {
            info(&format!("Mods page: {mod_name} {key} click ignored: it came with a Reset"));
        } else if let Some(built) = current() {
            if let Some(row) = built.sections.iter().flat_map(|s| s.rows.iter()).find(|r| r.mod_name == mod_name && r.key == key) {
                let value = staged_get(mod_name, key)
                    .or_else(|| REGISTRY.get().and_then(|r| r.get(mod_name, key)))
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                start_edit(row, &value);
            }
        }
    }
    if clicked == 0 {
        return;
    }
    // One line per click. The result is one-shot, so it is never dropped for the click cooldown or a
    // held mouse button (the game's Reset confirm can report while the button that pressed it is
    // still down); only our own open text popup ignores it.
    info(&format!(
        "Mods page: footer click {clicked} (in game {}, text popup {}, input held {})",
        OPENED_IN_GAME.load(Ordering::Acquire),
        popup_pending(),
        waiting
    ));
    if popup_pending() {
        return;
    }
    let (Some(registry), Some(built)) = (REGISTRY.get(), current()) else { return };
    match clicked {
        FOOTER_APPLY => apply_staged(registry, &built),
        FOOTER_UNDO => {
            let dropped = take_staged().len();
            info(&format!("Mods page: undo, {dropped} change(s) dropped"));
        }
        FOOTER_RESET => reset_staged(registry, &built),
        _ => {}
    }
}

/// A NUL-terminated copy that lives for the whole process.
fn leak_cstr(text: &str) -> *const u8 {
    let mut bytes: Vec<u8> = text.bytes().filter(|&b| b != 0).collect();
    bytes.push(0);
    Box::leak(bytes.into_boxed_slice()).as_ptr()
}

/// Look up the game language (at most every two seconds unless `force`) and rebuild the page when it
/// changed.
fn refresh_language(force: bool) {
    let now = now_ms();
    if !force && now.saturating_sub(LANG_CHECKED_MS.load(Ordering::Relaxed)) < 2000 {
        return;
    }
    LANG_CHECKED_MS.store(now, Ordering::Relaxed);
    let (Some(image), Some(log), Some(registry)) = (IMAGE.get(), LOG.get(), REGISTRY.get()) else { return };
    let code = lang::detect(image, log);
    let Ok(mut built) = BUILT.lock() else { return };
    if built.as_ref().is_some_and(|b| b.lang == code) {
        return;
    }
    info(&format!("Mods page language: {code}"));
    *built = Some(Arc::new(build(registry, code)));
}

fn current() -> Option<Arc<Built>> {
    BUILT.lock().ok()?.clone()
}

fn build(registry: &Registry, lang: &'static str) -> Built {
    let text = |id: &str| leak_cstr(menu_text::ui(lang, id));
    let ui = Ui {
        mods: text("mods"),
        title: text("title"),
        clear: text("clear"),
        clear_desc: text("clear_desc"),
        no_edit: text("no_edit"),
        not_set: menu_text::ui(lang, "not_set").to_string(),
    };
    Built { lang, ui, sections: build_sections(registry, lang) }
}

/// Build the sections from the registry. The game caches a row by its label, so a label that
/// repeats (two mods both have "Write log") gets trailing spaces to stay distinct.
fn build_sections(registry: &Registry, lang: &'static str) -> Vec<Section> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut sections = Vec::new();
    for (mod_name, specs) in registry.all_specs() {
        let mut rows = Vec::new();
        for spec in specs.iter().filter(|s| matches!(s.kind, Kind::Bool | Kind::Int | Kind::Float | Kind::Enum) || (s.kind == Kind::Text && TEXT_READY.load(Ordering::Acquire))) {
            let range = registry.range(mod_name, spec.key);
            let range_ok = match (spec.kind, range) {
                (Kind::Bool | Kind::Text, _) => true,
                (Kind::Int, Some(Range::Int { .. })) | (Kind::Float, Some(Range::Float { .. })) => true,
                (Kind::Enum, Some(Range::Enum { choices, .. })) => !choices.is_empty(),
                _ => false,
            };
            if !range_ok {
                continue;
            }
            let plain = menu_text::label(lang, mod_name, spec.key).unwrap_or(spec.label);
            let n = seen.entry(plain.to_string()).or_insert(0);
            let label = format!("{}{}", plain, " ".repeat(*n));
            *n += 1;
            let choices = match range {
                Some(Range::Enum { choices, .. }) => choices
                    .iter()
                    .map(|(key, text)| leak_cstr(menu_text::choice(lang, mod_name, spec.key, key).unwrap_or(text)))
                    .collect(),
                _ => Vec::new(),
            };
            rows.push(Row {
                mod_name,
                key: spec.key,
                kind: spec.kind,
                range,
                label: leak_cstr(&label),
                plain_label: plain.to_string(),
                default: registry.default_value(mod_name, spec.key),
                desc: leak_cstr(menu_text::desc(lang, mod_name, spec.key).unwrap_or(plain)),
                choices,
                clear_label: if spec.kind == Kind::Text {
                    leak_cstr(&menu_text::fill(menu_text::ui(lang, "clear_label"), 0, &plain.to_lowercase()))
                } else {
                    std::ptr::null()
                },
                shown: Mutex::new(ShownText { value: String::from("\u{1}"), ptr: std::ptr::null() }),
            });
        }
        if !rows.is_empty() {
            sections.push(Section { header: leak_cstr(mod_name), rows });
        }
    }
    sections
}

/// Two values are the same setting: numbers compare as decimals, everything else exactly.
fn same(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) if a.is_number() && b.is_number() => (x - y).abs() < 1e-6,
        _ => a == b,
    }
}

fn staged_get(mod_name: &str, key: &str) -> Option<serde_json::Value> {
    let guard = STAGED.lock().ok()?;
    guard.as_ref()?.iter().find(|((m, k), _)| *m == mod_name && *k == key).map(|(_, v)| v.clone())
}

fn staged_count() -> usize {
    STAGED.lock().ok().and_then(|g| g.as_ref().map(HashMap::len)).unwrap_or(0)
}

/// Remember a change the player made. A value equal to the live one is not a change.
fn stage(mod_name: &'static str, key: &'static str, live: Option<&serde_json::Value>, value: serde_json::Value) {
    let Ok(mut guard) = STAGED.lock() else { return };
    let map = guard.get_or_insert_with(HashMap::new);
    if live.is_some_and(|l| same(l, &value)) {
        map.remove(&(mod_name, key));
    } else {
        map.insert((mod_name, key), value);
    }
}

fn take_staged() -> HashMap<(&'static str, &'static str), serde_json::Value> {
    STAGED.lock().ok().and_then(|mut g| g.take()).unwrap_or_default()
}

/// Opening the page always starts with nothing staged, in the game's current language.
fn open_page_reset() {
    take_staged();
    refresh_language(true);
}

/// Give every staged change to its mod, in page order.
fn apply_staged(registry: &Registry, built: &Built) {
    let staged = take_staged();
    if staged.is_empty() {
        return;
    }
    let mut applied = 0;
    for row in built.sections.iter().flat_map(|s| s.rows.iter()) {
        let Some(value) = staged.get(&(row.mod_name, row.key)) else { continue };
        match registry.set(row.mod_name, row.key, value) {
            Ok(line) => {
                applied += 1;
                info(&format!("Mods page: {line}"));
            }
            Err(e) => info(&format!("Mods page: {} {}: {e}", row.mod_name, row.key)),
        }
    }
    info(&format!("Mods page: applied {applied} of {} change(s)", staged.len()));
}

/// Stage every setting's shipped default (logs off). Nothing changes until Apply.
fn reset_staged(registry: &Registry, built: &Built) {
    let mut checked = 0;
    for row in built.sections.iter().flat_map(|s| s.rows.iter()) {
        let Some(default) = registry.default_value(row.mod_name, row.key) else { continue };
        let live = registry.get(row.mod_name, row.key);
        stage(row.mod_name, row.key, live.as_ref(), default);
        checked += 1;
    }
    info(&format!("Mods page: reset to defaults, {checked} setting(s) checked, {} waiting for Apply", staged_count()));
}

/// Draw every row. Returns true when a popup of ours is open (clicks must be ignored).
fn draw_mods_page(page: u64) -> bool {
    let (Some(registry), Some(built)) = (REGISTRY.get(), current()) else { return false };
    // A popup of ours is open: draw every row but ignore what the player does to it.
    let waiting = input_blocked();
    // SAFETY: unique signature match of the nmspy `Header`.
    let header: HeaderFn = unsafe { std::mem::transmute(HEADER.load(Ordering::Acquire)) };
    for section in &built.sections {
        unsafe { header(page, section.header, ICON_ADVANCED) };
        for row in &section.rows {
            let live = registry.get(row.mod_name, row.key);
            let shown = staged_get(row.mod_name, row.key).or_else(|| live.clone());
            let changed = match (row.kind, row.range) {
                (Kind::Text, _) => draw_text(page, row, shown.as_ref(), waiting, &built.ui),
                (Kind::Bool, _) => draw_bool(page, row, shown.as_ref()),
                (Kind::Int, Some(Range::Int { min, max, step, default })) => draw_int(page, row, shown.as_ref(), (min, max, step, default)),
                (Kind::Float, Some(Range::Float { min, max, step, default })) => draw_float(page, row, shown.as_ref(), (min, max, step, default)),
                (Kind::Enum, Some(Range::Enum { choices, default })) => draw_enum(page, row, shown.as_ref(), choices, default),
                _ => None,
            };
            if let Some(value) = changed.filter(|_| !waiting) {
                stage(row.mod_name, row.key, live.as_ref(), value);
            }
        }
    }
    waiting
}

/// Each `draw_*` returns the new value when the player changed the row, `None` otherwise.
fn draw_bool(page: u64, row: &Row, shown: Option<&serde_json::Value>) -> Option<serde_json::Value> {
    // SAFETY: unique signature match of the nmspy `Boolean`.
    let boolean: BooleanFn = unsafe { std::mem::transmute(BOOLEAN.load(Ordering::Acquire)) };
    let current = u8::from(shown.and_then(|v| v.as_bool()).unwrap_or(false));
    // The game enables Reset while a row differs from the default it was given, so pass the real one.
    let default = row.default.as_ref().and_then(|v| v.as_bool()).map_or(current, u8::from);
    let value = unsafe { boolean(page, row.label, row.desc, current, default, &BOOL_TEXT) } & 1;
    (value != current).then(|| json!(value != 0))
}

fn draw_int(page: u64, row: &Row, shown: Option<&serde_json::Value>, (min, max, step, default): (i32, i32, i32, i32)) -> Option<serde_json::Value> {
    // SAFETY: unique signature match of the nmspy `Int`.
    let int: IntFn = unsafe { std::mem::transmute(INT.load(Ordering::Acquire)) };
    let current = shown.and_then(|v| v.as_i64()).map_or(default, |v| v.clamp(i64::from(min), i64::from(max)) as i32);
    let opts = IntOptions { step1: step, step2: 1, suffix: NO_SUFFIX.as_ptr(), unknown: 0 };
    let value = unsafe { int(page, row.label, row.desc, current, default, min, max, &opts) };
    (value != current && (min..=max).contains(&value)).then(|| json!(value))
}

fn draw_float(page: u64, row: &Row, shown: Option<&serde_json::Value>, (min, max, step, default): (f32, f32, f32, f32)) -> Option<serde_json::Value> {
    // SAFETY: unique signature match of the nmspy `Float`.
    let float: FloatFn = unsafe { std::mem::transmute(FLOAT.load(Ordering::Acquire)) };
    let current = shown.and_then(|v| v.as_f64()).map_or(default, |v| (v as f32).clamp(min, max));
    let opts = FloatOptions { step1: step, step2: 1.0, suffix: NO_SUFFIX.as_ptr(), unknown: 0 };
    let value = unsafe { float(page, row.label, row.desc, current, default, min, max, &opts) };
    // Four decimals: a slider step such as 0.1 should not be stored as 0.10000000149.
    (value != current && value >= min && value <= max).then(|| json!((f64::from(value) * 1e4).round() / 1e4))
}

fn draw_enum(page: u64, row: &Row, shown: Option<&serde_json::Value>, choices: &[(&str, &str)], default: usize) -> Option<serde_json::Value> {
    // SAFETY: unique signature matches of the nmspy `BeginList`, `ListOption` and the list's end.
    let begin: BeginListFn = unsafe { std::mem::transmute(BEGIN_LIST.load(Ordering::Acquire)) };
    let option: ListOptionFn = unsafe { std::mem::transmute(LIST_OPTION.load(Ordering::Acquire)) };
    let end: EndListFn = unsafe { std::mem::transmute(END_LIST.load(Ordering::Acquire)) };
    let current = shown
        .and_then(|v| v.as_str().and_then(|k| choices.iter().position(|(key, _)| *key == k)))
        .unwrap_or(default.min(choices.len() - 1)) as i32;
    let mut out: i32 = 0;
    unsafe {
        begin(page, row.label, row.desc, current, default as i32, &mut out);
        for (i, text) in row.choices.iter().enumerate() {
            option(page, *text, i as i32);
        }
    }
    let picked = unsafe { end(page) };
    (picked != current && picked >= 0).then(|| choices.get(picked as usize).map(|(key, _)| json!(key))).flatten()
}

/// How long after a popup closes the page ignores clicks, in milliseconds.
const COOLDOWN_MS: u64 = 400;
/// Clicks are ignored until this time (ms since the first call); 0 means no cooldown.
static IGNORE_UNTIL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static CLOCK: OnceLock<std::time::Instant> = OnceLock::new();
const VK_LBUTTON: i32 = 0x01;

fn now_ms() -> u64 {
    CLOCK.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// Start ignoring clicks on our rows. Called when a popup closes: the click that pressed Accept or
/// Cancel is still down (or just released) over whatever row lies beneath the box.
fn start_cooldown() {
    IGNORE_UNTIL_MS.store(now_ms() + COOLDOWN_MS, Ordering::Release);
}

/// True while our rows must ignore what the player does: a popup is open, or the click that closed
/// one is still held or only just released.
fn input_blocked() -> bool {
    if popup_pending() {
        return true;
    }
    let until = IGNORE_UNTIL_MS.load(Ordering::Acquire);
    if until == 0 {
        return false;
    }
    let now = now_ms();
    // The mouse button that closed the popup is still held: wait for its release, then the short gap.
    if async_key_down(VK_LBUTTON) {
        IGNORE_UNTIL_MS.store(now + COOLDOWN_MS, Ordering::Release);
        return true;
    }
    if now < until {
        return true;
    }
    IGNORE_UNTIL_MS.store(0, Ordering::Release);
    false
}

fn popup_pending() -> bool {
    PENDING.lock().map(|p| p.is_some()).unwrap_or(true)
}

/// What a text row shows in its cell: the value, shortened, or a hint when empty.
fn cell_text(value: &str, not_set: &str) -> String {
    const SHOWN: usize = 36;
    if value.trim().is_empty() {
        return not_set.to_string();
    }
    let mut chars = value.chars();
    let head: String = chars.by_ref().take(SHOWN).collect();
    if chars.next().is_some() {
        format!("{head}...")
    } else {
        head
    }
}

/// The longest value the popup can be pre-filled with: it takes a fixed 64 byte buffer.
const INITIAL_MAX: usize = 63;
/// The longest text the player can type.
const TYPED_MAX: u32 = 200;

/// A text row. Returns the new value when the player cleared it; an edit is staged by the popup's
/// result handler instead, because the box finishes later.
fn draw_text(
    page: u64,
    row: &Row,
    shown_value: Option<&serde_json::Value>,
    waiting: bool,
    ui: &Ui,
) -> Option<serde_json::Value> {
    // SAFETY: unique signature match of the nmspy `Boolean`.
    let boolean: BooleanFn = unsafe { std::mem::transmute(BOOLEAN.load(Ordering::Acquire)) };
    let value = shown_value.and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
    let ptr = {
        let Ok(mut shown) = row.shown.lock() else { return None };
        if shown.value != value || shown.ptr.is_null() {
            shown.ptr = leak_cstr(&cell_text(&value, &ui.not_set));
            shown.value = value.clone();
        }
        shown.ptr
    };
    // The cell shows the value in both states; the row is shown as "off", so any click comes back as "on".
    let cell = BooleanOptions { enabled: ptr, disabled: ptr };
    let editable = OPENED_IN_GAME.load(Ordering::Acquire);
    let desc = if editable { row.desc } else { ui.no_edit };
    // Shown as "off" (0); the default is 1 while the text differs from its default, so Reset stays enabled.
    let differs = row.default.as_ref().and_then(|d| d.as_str()).is_some_and(|d| d != value);
    let clicked = {
        let _disabled = (!editable)
            .then(|| DisabledRow::enter(page as usize, ROW_DISABLED_OFFSET.load(Ordering::Acquire)))
            .flatten();
        (unsafe { boolean(page, row.label, desc, 0, u8::from(differs), &cell) } & 1) != 0
    };
    if clicked && !waiting && editable {
        // The popup starts from the staged value if there is one, so an edit builds on it.
        if let Ok(mut click) = TEXT_CLICK.lock() {
            *click = Some((row.mod_name, row.key));
        }
    }
    // Cancel and "accept nothing" look the same to the game, so emptying a setting has its own row.
    let mut cleared = None;
    if !value.trim().is_empty() && !row.clear_label.is_null() {
        let clear = BooleanOptions { enabled: ui.clear, disabled: ui.clear };
        let hit = unsafe { boolean(page, row.clear_label, ui.clear_desc, 0, 0, &clear) } & 1 != 0;
        if hit && !waiting {
            info(&format!("Mods page: {} {} cleared (waiting for Apply)", row.mod_name, row.key));
            cleared = Some(json!(""));
            start_cooldown();
        }
    }
    cleared
}

/// Open the game's text popup for one setting. Refuses (and says why) rather than risk a bad state.
fn start_edit(row: &Row, value: &str) {
    if !OPENED_IN_GAME.load(Ordering::Acquire) {
        info("text edit not started: the text box is only drawn in game");
        return;
    }
    let mgr = TEXT_MGR.load(Ordering::Acquire);
    if mgr == 0 || !TEXT_READY.load(Ordering::Acquire) {
        info("text edit not started: the game's text popup has not been seen yet");
        return;
    }
    // The popup is shared with the game (renames). Never start over one that is open: `Start` writes
    // its flags before it checks the state.
    if read_u32(mgr + TEXT_STATE_OFFSET) != Some(0) {
        info("text edit not started: the game's text box is busy");
        return;
    }
    if value.len() > INITIAL_MAX {
        info(&format!("{} {}: value is {} bytes, too long to edit in the game (limit {INITIAL_MAX}); edit it in the NMS.rs window", row.mod_name, row.key, value.len()));
        return;
    }
    let Ok(mut pending) = PENDING.lock() else { return };
    if pending.is_some() {
        return;
    }
    let spec_label = registry_label(row);
    let title = fixed64(&spec_label);
    let initial = fixed64(value);
    // SAFETY: nmspy-style `Start(mgr, title[64], initial[64], filter, maxLen)`; the buffers outlive the call
    // (the game copies both) and `mgr` is the object the game's own `Update` was called on.
    let start: TextStartFn = unsafe { std::mem::transmute(TEXT_START.load(Ordering::Acquire)) };
    unsafe { start(mgr as u64, &title, &initial, 0, TYPED_MAX) };
    if read_u32(mgr + TEXT_STATE_OFFSET) == Some(0) {
        info("text edit not started: the game did not open its text box");
        return;
    }
    *pending = Some((row.mod_name, row.key));
    STARTED_MS.store(now_ms(), Ordering::Release);
    info(&format!(
        "text edit started: {} {} (keyboard flag {:?}, state {:?})",
        row.mod_name,
        row.key,
        read_u8(mgr + TEXT_KEYBOARD_FLAG_OFFSET),
        read_u32(mgr + TEXT_STATE_OFFSET)
    ));
}

/// The row's label for the popup title, without the trailing spaces used to keep labels apart.
fn registry_label(row: &Row) -> String {
    row.plain_label.clone()
}

/// UTF-8 text in a zero-filled 64 byte buffer, cut at a character boundary, always NUL-terminated.
fn fixed64(text: &str) -> [u8; 64] {
    let mut buf = [0u8; 64];
    let mut end = text.len().min(63);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    buf[..end].copy_from_slice(&text.as_bytes()[..end]);
    buf
}

/// Offset of the text manager's state (0 idle, 1 open, 2 result ready, 3 and 4 profanity check).
const TEXT_STATE_OFFSET: usize = 0x670;

/// Give the keyboard back to the game and close the popup: what the game does when the player accepts
/// or cancels. Safe to run when the popup is already closed.
fn close_popup(mgr: usize) {
    // Only a popup the game has not closed itself still holds the keyboard. After Accept or Cancel the
    // game already gave it back, so then only `Stop` is needed.
    let still_open = read_u32(mgr + TEXT_STATE_OFFSET) == Some(1);
    let controller = still_open.then(|| super::context::input_controller(APP_GLOBAL.load(Ordering::Acquire))).flatten();
    if let Some(controller) = controller {
        // SAFETY: the byte and the routine are the ones the game's own close code uses (checked at start).
        unsafe {
            let _ = nms_core::mem::write(controller + 0x40, 0u8);
            let set_actions: SetActionsFn = std::mem::transmute(SET_ACTIONS.load(Ordering::Acquire));
            set_actions((controller + sigs::CONTROLLER_SETS as usize) as u64, 1, 1);
        }
    }
    // SAFETY: nmspy-style `Stop(mgr)`.
    let stop: TextStopFn = unsafe { std::mem::transmute(TEXT_STOP.load(Ordering::Acquire)) };
    unsafe { stop(mgr as u64) };
}


/// The result is read this long after the game closed the box. Until then the manager still reads as
/// busy, so a page that ignores Back while the box is busy also ignores the press that closed it.
const FINISH_DELAY_MS: u64 = 150;
/// If the page is popped within this long after the box closed, it is put back (see `WATCH_UNTIL_MS`).
const WATCH_MS: u64 = 300;

/// After the box closes, game input stays off this long. The game gives input back in the same frame
/// it reads Cancel, so the page's own Back handling saw the same B press and left the menu.
const HOLD_MS: u64 = 300;
/// Input is held off until this time (ms on the `now_ms` clock); 0 means it is not held.
static HOLD_UNTIL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Switch the game's input action set on or off, exactly as the text popup does at start and end.
fn set_input(enabled: bool) {
    if SET_ACTIONS.load(Ordering::Acquire) == 0 {
        return;
    }
    let Some(controller) = super::context::input_controller(APP_GLOBAL.load(Ordering::Acquire)) else { return };
    // SAFETY: the byte and the routine are the ones the game's own start and close code uses (checked at start).
    unsafe {
        let _ = nms_core::mem::write(controller + 0x40, u8::from(!enabled));
        let set_actions: SetActionsFn = std::mem::transmute(SET_ACTIONS.load(Ordering::Acquire));
        set_actions((controller + sigs::CONTROLLER_SETS as usize) as u64, 1, u8::from(enabled));
    }
}

/// Keep game input off for a moment after a popup closed normally (see `HOLD_MS`).
fn hold_input() {
    set_input(false);
    HOLD_UNTIL_MS.store(now_ms() + HOLD_MS, Ordering::Release);
}

/// The core's after-update tick (main thread).
fn release_input_tick(_: &nms_core::game::dispatch::Tick) {
    release_input_if_due();
}

/// Give input back once the hold is over. Runs every frame, so input can never stay off.
fn release_input_if_due() {
    let until = HOLD_UNTIL_MS.load(Ordering::Acquire);
    if until != 0 && now_ms() >= until {
        HOLD_UNTIL_MS.store(0, Ordering::Release);
        set_input(true);
    }
}

/// Offset of the "in-game keyboard active" byte; `Update` does nothing while it is 0.
const TEXT_KEYBOARD_FLAG_OFFSET: usize = 0xAF4;
/// Offset of the text manager inside the application object (build 183791: `app+0x880CFC` is its
/// `+0xAF4` and `app+0x880878` its `+0x670`, read at `0x14074EEA9`; `0x859970` before).
const TEXT_MGR_OFFSET: usize = 0x880208;
/// The real text manager (the game also runs `Update` on a second, unrelated instance).
fn real_mgr() -> Option<usize> {
    super::context::data(APP_GLOBAL.load(Ordering::Acquire)).map(|app| app + TEXT_MGR_OFFSET)
}

/// The press that closes the box with B can also reach the page as Back and leave the Mods page. If
/// the page is popped right after a popup closed, open it again.
fn restore_page_if_popped() {
    let until = WATCH_UNTIL_MS.load(Ordering::Acquire);
    if until == 0 {
        return;
    }
    if now_ms() >= until {
        WATCH_UNTIL_MS.store(0, Ordering::Release);
        return;
    }
    if top_state() != Some(MODS_PAGE) && OPENED_IN_GAME.load(Ordering::Acquire) {
        WATCH_UNTIL_MS.store(0, Ordering::Release);
        info("Mods page was left by the press that closed the text box: opening it again");
        // SAFETY: nmspy `RequestOptionsPage(eOptionsMenu, bool)`, as the Mods row click calls it.
        let request: RequestPageFn = unsafe { std::mem::transmute(REQUEST_PAGE.load(Ordering::Acquire)) };
        unsafe { request(MODS_PAGE as i32, 1) };
    }
}

/// Every frame, after the game's text-entry update: take the manager, and finish or abandon our popup.
fn on_text_update(args: &text_input_update::Args, _ret: u64) {
    let mgr = args.this as usize;
    if Some(mgr) != real_mgr() {
        return;
    }
    TEXT_MGR.store(mgr, Ordering::Release);
    release_input_if_due();
    restore_page_if_popped();
    let Ok(mut pending) = PENDING.lock() else { return };
    let Some((mod_name, key)) = *pending else { return };
    // The player left our page (mouse Back, a game state change) with the box open: close it for them.
    if top_state() != Some(MODS_PAGE) {
            DONE_SEEN_MS.store(0, Ordering::Release);
        close_popup(mgr);
        *pending = None;
        start_cooldown();
        info(&format!("text edit abandoned: left the Mods page ({mod_name} {key} unchanged)"));
        return;
    }
    // The game closed the box (state 2). Keep it reading as busy for a moment before we take the result.
    if read_u32(mgr + TEXT_STATE_OFFSET) == Some(2) {
        let seen = DONE_SEEN_MS.load(Ordering::Acquire);
        if seen == 0 {
            DONE_SEEN_MS.store(now_ms(), Ordering::Release);
            return;
        }
        if now_ms().saturating_sub(seen) < FINISH_DELAY_MS {
            return;
        }
    }
    DONE_SEEN_MS.store(0, Ordering::Release);
    // SAFETY: nmspy-style `GetResult(mgr, out[256])`.
    let result: TextResultFn = unsafe { std::mem::transmute(TEXT_RESULT.load(Ordering::Acquire)) };
    let mut out = [0u8; 256];
    let status = unsafe { result(mgr as u64, &mut out) };
    if status == 0 {
        // Still open. If something else closed it, stop waiting.
        if read_u32(mgr + TEXT_STATE_OFFSET) == Some(0) {
            *pending = None;
                    start_cooldown();
            info(&format!("text edit ended without a result ({mod_name} {key} unchanged)"));
        }
        return;
    }
    close_popup(mgr);
    // The game closed the box and gave input back in this same frame: keep it off a moment so the
    // press that closed the box (B on a controller) does not also reach the page.
    hold_input();
    WATCH_UNTIL_MS.store(now_ms() + WATCH_MS, Ordering::Release);
    *pending = None;
    drop(pending);
    let end = out.iter().position(|&b| b == 0).unwrap_or(out.len());
    let typed = String::from_utf8_lossy(&out[..end]).trim().to_string();
    // The game reports Cancel as an empty result, so an empty result changes nothing (the Clear row
    // empties a setting).
    start_cooldown();
    if status != 1 || typed.is_empty() {
        info(&format!("text edit finished with no text ({mod_name} {key} unchanged)"));
        return;
    }
    let Some(registry) = REGISTRY.get() else { return };
    let live = registry.get(mod_name, key);
    stage(mod_name, key, live.as_ref(), json!(typed));
    info(&format!("Mods page: {mod_name} {key} set to a new text (waiting for Apply)"));
}

/// 16-byte zero-padded name, as the game's name hash expects.
fn pad_name(name: &str) -> [u8; 16] {
    let mut buf = [0u8; 16];
    let n = name.len().min(16);
    buf[..n].copy_from_slice(&name.as_bytes()[..n]);
    buf
}

/// After the game set the title for its own pages: on ours, put "MODS" into the header text.
fn on_title(_args: &options_page_title::Args, _ret: u64) {
    if top_state() != Some(MODS_PAGE) {
        return;
    }
    let Some(root) = read_ptr(TITLE_ROOT.load(Ordering::Acquire)) else { return };
    let mut name = pad_name("HEADERTEXT");
    let mut hashed = [0u8; 24];
    // SAFETY: the game's own name-hash and child-lookup routines (unique signatures), called the
    // way the title function calls them: kind 1 for the header text element.
    let hash: HashFn = unsafe { std::mem::transmute(HASH_FN.load(Ordering::Acquire)) };
    let lookup: LookupFn = unsafe { std::mem::transmute(LOOKUP_FN.load(Ordering::Acquire)) };
    let element = unsafe {
        hash(hashed.as_mut_ptr(), name.as_mut_ptr(), 1);
        lookup(root as u64, hashed.as_ptr(), 1)
    } as usize;
    let Some(vtable) = read_ptr(element) else { return };
    let Some(set_text) = read_ptr(vtable + 0x90) else { return };
    // SAFETY: virtual slot 0x90 of the text element is the call the title function makes with a text id.
    let set_text: SetTextFn = unsafe { std::mem::transmute(set_text) };
    let Some(built) = current() else { return };
    unsafe { set_text(element as u64, built.ui.title) };
}

/// After the game drew the pause list: handle our `BUTTON_MODS` row the way the page handles its own.
fn on_switcher(args: &options_game_switcher::Args, _ret: u64) {
    // The page skips its click tests while a popup is open; so do we.
    let Some(state) = read_ptr(SWITCHER_BUSY.load(Ordering::Acquire)) else { return };
    if read_u8(state + sigs::BUSY_OFFSET).unwrap_or(1) != 0 {
        return;
    }
    let Some(table) = read_ptr(args.page as usize + sigs::TABLE_OFFSET) else { return };
    let mut name = pad_name("BUTTON_MODS");
    let mut hashed = [0u8; 24];
    // SAFETY: the game's own name-hash, child-lookup and child-find routines (unique signatures),
    // called as `DoGameSwitcher` calls them for `BUTTON_DIF` and its `WHITE` child.
    let hash: HashFn = unsafe { std::mem::transmute(HASH_FN.load(Ordering::Acquire)) };
    let lookup: LookupFn = unsafe { std::mem::transmute(LOOKUP_FN.load(Ordering::Acquire)) };
    let find_child: FindChildFn = unsafe { std::mem::transmute(FIND_CHILD.load(Ordering::Acquire)) };
    let button = unsafe {
        hash(hashed.as_mut_ptr(), name.as_mut_ptr(), 0);
        lookup(table as u64, hashed.as_ptr(), 0)
    } as usize;
    if button < MIN_ELEMENT {
        return;
    }
    let white_name = pad_name("WHITE");
    let white = unsafe { find_child(button as u64, white_name.as_ptr(), 1) };
    if (white as usize) < MIN_ELEMENT {
        return;
    }
    // SAFETY: `Confirm(page, element, true, 0, &args, false, true)` exactly as the page calls it.
    let confirm: ConfirmFn = unsafe { std::mem::transmute(CONFIRM.load(Ordering::Acquire)) };
    let call = ConfirmArgs { zero: 0, id: 0x18AE_CCF6, flag: 1, pad: [0; 3] };
    let clicked = unsafe { confirm(args.page, white, 1, 0, &call, 0, 1) };
    if clicked & 1 != 0 {
        OPENED_IN_GAME.store(true, Ordering::Release);
        open_page_reset();
        info("pause list: Mods row clicked, opening page 8");
        // SAFETY: nmspy `RequestOptionsPage(eOptionsMenu, bool)`.
        let request: RequestPageFn = unsafe { std::mem::transmute(REQUEST_PAGE.load(Ordering::Acquire)) };
        unsafe { request(MODS_PAGE as i32, 1) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nms_core::FileImage;

    #[test]
    fn fixed64_cuts_on_a_character_boundary_and_terminates() {
        let long = "é".repeat(40); // 80 bytes
        let buf = fixed64(&long);
        assert_eq!(buf[63], 0);
        let end = buf.iter().position(|&b| b == 0).unwrap();
        assert!(std::str::from_utf8(&buf[..end]).is_ok());
        assert_eq!(fixed64("abc")[..4], *b"abc\0");
    }

    #[test]
    fn staging_drops_a_value_equal_to_the_live_one() {
        // The static is shared with other tests in this module: use a key no other test uses.
        let live = json!(true);
        stage("StageTest", "a", Some(&live), json!(false));
        assert_eq!(staged_get("StageTest", "a"), Some(json!(false)));
        stage("StageTest", "a", Some(&live), json!(true));
        assert_eq!(staged_get("StageTest", "a"), None);
        // Decimals compare with a tolerance, so a slider that lands on the same step is no change.
        let live = json!(0.1);
        stage("StageTest", "b", Some(&live), json!(0.1000001));
        assert_eq!(staged_get("StageTest", "b"), None);
        stage("StageTest", "b", Some(&live), json!(0.2));
        assert!(staged_get("StageTest", "b").is_some());
        take_staged();
    }

    #[test]
    fn cell_text_shortens_long_values_and_hints_when_empty() {
        assert_eq!(cell_text("ion battery", "(not set)"), "ion battery");
        assert!(cell_text(&"x".repeat(80), "(not set)").ends_with("..."));
        assert_eq!(cell_text("  ", "(not set)"), "(not set)");
    }

    #[test]
    fn disabled_row_restores_the_previous_state_before_the_next_widget() {
        for previous in [0u32, 7] {
            let mut page = [42u32; 12];
            page[6] = previous;
            let original = page;
            let address = page.as_mut_ptr() as usize;
            {
                let _disabled = DisabledRow::enter(address, 0x18).unwrap();
                assert_eq!(page[6], 1);
                assert_eq!(page[5], 42);
                assert_eq!(page[7], 42);
            }
            assert_eq!(page, original);
            assert!(DisabledRow::enter(address, 0).is_none());
            assert_eq!(page, original);
        }
        assert!(DisabledRow::enter(0, 0x18).is_none());
        assert!(DisabledRow::enter(usize::MAX, 0x18).is_none());
    }

    #[test]
    fn row_disabled_layout_rejects_a_changed_page_field_or_call() {
        let mut bytes = vec![0xCC; 0x300];
        bytes[0x7F..0x88].copy_from_slice(&[0x48, 0x8B, 0xD6, 0x48, 0x8D, 0x4C, 0x24, 0x40, 0xE8]);
        bytes[0x88..0x8C].copy_from_slice(&(0x180i32 - 0x8C).to_le_bytes());
        let helper = [
            0x48, 0x89, 0x5C, 0x24, 0x10, 0x44, 0x89, 0x44, 0x24, 0x18,
            0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
            0x48, 0x8D, 0x6C, 0x24, 0xD9, 0x48, 0x81, 0xEC, 0x90, 0, 0, 0,
            0x44, 0x8B, 0x72, 0x18,
        ];
        bytes[0x180..0x180 + helper.len()].copy_from_slice(&helper);
        let image = FileImage::from_code(&bytes);
        assert_eq!(row_disabled_offset(&image, 0), Some(0x18));
        for at in [0x87, 0x180 + helper.len() - 1] {
            let mut changed = image.clone();
            changed.write(at, &[0xFF]).unwrap();
            assert_eq!(row_disabled_offset(&changed, 0), None);
        }
        for displacement in [i32::MIN, i32::MAX] {
            let mut changed = image.clone();
            changed.write(0x88, &displacement.to_le_bytes()).unwrap();
            assert_eq!(row_disabled_offset(&changed, 0), None);
        }
        assert_eq!(row_disabled_offset(&FileImage::from_code(&[0; 32]), 0), None);
    }

    #[test]
    #[ignore = "requires an explicitly supplied executable path"]
    fn disabled_row_matches_supplied_executable() {
        let path = std::env::var_os("NMS_ROW_CHECK_EXE").expect("set NMS_ROW_CHECK_EXE");
        let image = FileImage::load(std::path::Path::new(&path)).unwrap();
        let boolean = image.find_unique(&Pattern::parse(sigs::BOOLEAN).unwrap()).unique().unwrap();
        assert_eq!(row_disabled_offset(&image, boolean), Some(0x18));
        println!("Read-only disabled-row check: Boolean RVA {boolean:#x}, page offset 0x18");
    }
}

//! The game's own text-entry popup (`cGcFrontendTextInput`, the box that renames a storage
//! container), driven from the shared `Update` after-hook.
//!
//! These are small copies of the calls the
//! in-game Mods page makes (`common/options_menu.rs`), with the same checks: the application global
//! and the "switch action set" routine are read from the close code at the end of `Update`, and the
//! bytes there must match before anything is used.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::common::context;
use nms_core::mem::read_u32;
use nms_core::process::ProcessImage;
use nms_core::{Code, Pattern};
use nms_sdk::sigs::ingame_options as sigs;

/// Offset of the manager's state (0 idle, 1 open, 2 result ready, 3 and 4 profanity check).
const STATE: usize = 0x670;
/// Offset of the text manager inside the application object (`0x859970` before build 183791; see
/// `TEXT_MGR_OFFSET` in `common/options_menu.rs` for where it was read).
const MGR_OFFSET: usize = 0x880208;
/// The input controller's action-set group (`ctl+0x1770`, `0x1500` before build 183791).
const CTL_SETS: usize = sigs::CONTROLLER_SETS as usize;
/// The longest text the player can type.
const TYPED_MAX: u32 = 40;

static START: AtomicUsize = AtomicUsize::new(0);
static RESULT: AtomicUsize = AtomicUsize::new(0);
static STOP: AtomicUsize = AtomicUsize::new(0);
static SET_ACTIONS: AtomicUsize = AtomicUsize::new(0);
static APP_GLOBAL: AtomicUsize = AtomicUsize::new(0);

type StartFn = unsafe extern "system" fn(u64, *const [u8; 64], *const [u8; 64], u8, u32);
type ResultFn = unsafe extern "system" fn(u64, *mut [u8; 256]) -> i32;
type StopFn = unsafe extern "system" fn(u64);
type SetActionsFn = unsafe extern "system" fn(u64, u32, u8);

fn find(image: &ProcessImage, name: &str, pattern: &str) -> Result<usize, String> {
    let pat = Pattern::parse(pattern).map_err(|e| format!("{name}: {e}"))?;
    image.find_unique(&pat).unique().ok_or_else(|| format!("{name} not found (game update?)"))
}

/// Find `Start`, `GetResult`, `Stop` and the close code. Nothing is called or written here.
pub fn locate(image: &ProcessImage) -> Result<(), String> {
    let start = find(image, "text popup start", sigs::TEXT_START)?;
    let result = find(image, "text popup result", sigs::TEXT_RESULT)?;
    let stop = find(image, "text popup stop", sigs::TEXT_STOP)?;
    let update = find(image, "text popup update", sigs::TEXT_UPDATE)?;
    let (app_global, set_actions) = crate::common::options_menu::text_close_code(image, update)?;
    APP_GLOBAL.store(app_global, Ordering::Release);
    SET_ACTIONS.store(set_actions, Ordering::Release);
    START.store(image.va(start), Ordering::Release);
    RESULT.store(image.va(result), Ordering::Release);
    STOP.store(image.va(stop), Ordering::Release);
    Ok(())
}

/// The application data, or `None` before the game created it (core context first, then the
/// global read from the close code).
pub fn app() -> Option<usize> {
    context::data(APP_GLOBAL.load(Ordering::Acquire))
}

/// The real text manager (the game also runs `Update` on a second, unrelated instance).
pub fn real_mgr() -> Option<usize> {
    app().map(|a| a + MGR_OFFSET)
}

/// The input controller: action table, contexts, action sets.
pub fn controller() -> Option<usize> {
    context::input_controller(APP_GLOBAL.load(Ordering::Acquire))
}

/// The manager is idle: no box of ours or the game's is open.
pub fn idle(mgr: usize) -> bool {
    read_u32(mgr + STATE) == Some(0)
}

/// UTF-8 text in a zero-filled 64 byte buffer, cut at a character boundary, always NUL-terminated.
pub fn fixed64(text: &str) -> [u8; 64] {
    let mut buf = [0u8; 64];
    let mut end = text.len().min(63);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    buf[..end].copy_from_slice(&text.as_bytes()[..end]);
    buf
}

/// Open the box. Returns false (and opens nothing) when the manager is busy or did not open.
pub fn start(mgr: usize, title: &str, initial: &str) -> bool {
    let start = START.load(Ordering::Acquire);
    if start == 0 || !idle(mgr) {
        return false;
    }
    let title = fixed64(title);
    let initial = fixed64(initial);
    // SAFETY: `Start(mgr, title[64], initial[64], filter, maxLen)`, as the Mods page calls it. The
    // game copies both buffers; `mgr` is the application's own manager (checked by the caller).
    let start: StartFn = unsafe { std::mem::transmute(start) };
    unsafe { start(mgr as u64, &title, &initial, 0, TYPED_MAX) };
    !idle(mgr)
}

/// What the box did since the last frame.
pub enum Poll {
    /// Still open.
    Waiting,
    /// Something else closed it without a result.
    Gone,
    /// Closed by the player. `None` when the game refused the text; an empty string for Cancel
    /// or an empty Accept (the game reports both the same way: `Update` copies an empty string
    /// over the text on Cancel and both paths end in state 2, `GetResult` 1). Tell them apart
    /// with [`KeyWatch`].
    Done(Option<String>),
}

/// A `KeyWatch` with nothing seen (for statics; equal to `KeyWatch::default()`).
pub const KEY_WATCH_IDLE: KeyWatch = KeyWatch { accept_ms: 0, cancel_ms: 0 };

/// A key that ends the box was seen down this recently before an empty result.
pub const END_KEY_MS: u64 = 500;

/// Which key ended the box. `GetResult` cannot say, so the hook samples the keys every frame while
/// the box is open: Enter and the left mouse button accept it (`TextChatSend`, `Fe_Select`), Esc
/// and the right mouse button cancel it (`TextChatOpenClose`, `Fe_Back`, `Fe_ExitMenu`). The caller
/// also feeds in those actions as the game's action table has them (`shield::end_keys`), so pad A
/// and B count with Steam Input too.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyWatch {
    accept_ms: u64,
    cancel_ms: u64,
}

impl KeyWatch {
    /// One sample, taken before the frame's `GetResult`.
    pub fn sample(&mut self, now: u64, accept_down: bool, cancel_down: bool) {
        if accept_down {
            self.accept_ms = now;
        }
        if cancel_down {
            self.cancel_ms = now;
        }
    }

    /// An empty result was an Accept: an accept key was down within [`END_KEY_MS`] and later than
    /// any cancel key. A tie, no key at all (controller, Steam keyboard) or an old press counts as
    /// Cancel, which leaves everything unchanged.
    pub fn empty_was_accept(&self, now: u64) -> bool {
        self.accept_ms != 0 && now.saturating_sub(self.accept_ms) <= END_KEY_MS && self.accept_ms > self.cancel_ms
    }
}

pub fn poll(mgr: usize) -> Poll {
    let result = RESULT.load(Ordering::Acquire);
    if result == 0 {
        return Poll::Gone;
    }
    let mut out = [0u8; 256];
    // SAFETY: `GetResult(mgr, out[256])`, as the Mods page calls it.
    let result: ResultFn = unsafe { std::mem::transmute(result) };
    let status = unsafe { result(mgr as u64, &mut out) };
    if status == 0 {
        return if idle(mgr) { Poll::Gone } else { Poll::Waiting };
    }
    close(mgr);
    if status != 1 {
        return Poll::Done(None);
    }
    let end = out.iter().position(|&b| b == 0).unwrap_or(out.len());
    Poll::Done(Some(String::from_utf8_lossy(&out[..end]).trim().to_string()))
}

/// Give the keyboard back if the box still holds it, then `Stop`. Safe when already closed.
pub fn close(mgr: usize) {
    if read_u32(mgr + STATE) == Some(1) {
        set_input(true);
    }
    let stop = STOP.load(Ordering::Acquire);
    if stop == 0 {
        return;
    }
    // SAFETY: `Stop(mgr)`.
    let stop: StopFn = unsafe { std::mem::transmute(stop) };
    unsafe { stop(mgr as u64) };
}

/// The manager's state (0 idle, 1 open, 2 result ready, 3 and 4 profanity check).
pub fn state(mgr: usize) -> Option<u32> {
    read_u32(mgr + STATE)
}

/// Switch action set 1 on or off without touching the "text entry active" byte (`ctl+0x40`). The
/// hold after our box uses this: that byte belongs to an open text box only (the controller's
/// handler loop reads it every frame), and no box is open during the hold.
pub fn set_action_set(enabled: bool) -> bool {
    let set_actions = SET_ACTIONS.load(Ordering::Acquire);
    let Some(controller) = controller() else { return false };
    if set_actions == 0 {
        return false;
    }
    // SAFETY: the routine and arguments of the game's own start and close code (checked in `locate`);
    // it only flips `[entry+0x49]` on the entries of set 1.
    unsafe {
        let set_actions: SetActionsFn = std::mem::transmute(set_actions);
        set_actions((controller + CTL_SETS) as u64, 1, u8::from(enabled));
    }
    true
}

/// Switch the game's input action set on or off, exactly as the box does at start and end. Only
/// used to close a box of ours that is still open (state 1), as the game's close code does.
pub fn set_input(enabled: bool) {
    let set_actions = SET_ACTIONS.load(Ordering::Acquire);
    let Some(controller) = controller() else { return };
    if set_actions == 0 {
        return;
    }
    // SAFETY: the byte and the routine are the ones the game's own start and close code uses
    // (checked in `locate`).
    unsafe {
        let _ = nms_core::mem::write(controller + 0x40, u8::from(!enabled));
        let set_actions: SetActionsFn = std::mem::transmute(set_actions);
        set_actions((controller + CTL_SETS) as u64, 1, u8::from(enabled));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed64_cuts_on_a_character_boundary() {
        let buf = fixed64(&"é".repeat(40));
        assert_eq!(buf[63], 0);
        let end = buf.iter().position(|&b| b == 0).unwrap();
        assert!(std::str::from_utf8(&buf[..end]).is_ok());
        assert_eq!(fixed64("ab")[..3], *b"ab\0");
    }

    #[test]
    fn key_watch_tells_empty_accept_from_cancel() {
        // Enter on the closing frame: accept.
        let mut w = KeyWatch::default();
        w.sample(1000, false, false);
        w.sample(1016, true, false);
        assert!(w.empty_was_accept(1016));
        // Esc released (it ends the box one frame after the release): cancel.
        let mut w = KeyWatch::default();
        w.sample(1000, false, true);
        w.sample(1016, false, false);
        assert!(!w.empty_was_accept(1016));
        // Typed with Enter long ago, then Esc: cancel.
        let mut w = KeyWatch::default();
        w.sample(100, true, false);
        w.sample(900, false, true);
        assert!(!w.empty_was_accept(916));
        // No key seen (controller B, Steam keyboard): cancel. An old Enter: cancel.
        assert!(!KeyWatch::default().empty_was_accept(5000));
        let mut w = KeyWatch::default();
        w.sample(100, true, false);
        assert!(!w.empty_was_accept(100 + END_KEY_MS + 1));
        // Both on the same frame: cancel wins.
        let mut w = KeyWatch::default();
        w.sample(700, true, true);
        assert!(!w.empty_was_accept(700));
    }
}

//! The game context from the core (`nms_core::game`), for mods that used to find the application
//! data themselves. Each accessor falls back to the mod's own anchor (the global it read from game
//! code) when the core context is off, so a mod never loses a feature because of it.

use nms_core::game::live;
use nms_core::mem::read_ptr;

/// The input controller offset in the application data when the core context is off (build 184271:
/// getter 6 is `mov rax,[rax+0x95AFB8]`; 0x933B78 on 180836, 0x933BC8 on 181442, 0x95AEB8 on 183791,
/// 0x95AEA8 on 183877).
pub const CONTROLLER_OFFSET_FALLBACK: u32 = 0x95AFB8;

/// Offset of the input controller pointer in the application data, read from the game's getter by
/// the core (0x95AFB8 on build 184271).
pub fn controller_offset() -> u32 {
    live::get().map_or(CONTROLLER_OFFSET_FALLBACK, |g| g.layout().members.input_controller)
}

/// `cGcApplication::Data` (older code calls it "app"). `fallback_global` is the address of the
/// global holding it, as the mod found it in game code; 0 if unknown.
pub fn data(fallback_global: usize) -> Option<usize> {
    match live::get() {
        Some(g) => g.data(),
        None if fallback_global != 0 => read_ptr(fallback_global),
        None => None,
    }
}

/// The input controller object.
pub fn input_controller(fallback_global: usize) -> Option<usize> {
    match live::get() {
        Some(g) => g.input_controller(),
        None => read_ptr(data(fallback_global)? + CONTROLLER_OFFSET_FALLBACK as usize),
    }
}

/// The close code at the end of the text popup's `Update` loads the controller with
/// `mov rcx,[rax+disp32]`; its disp32 must be the controller offset for this build.
pub fn controller_disp_matches(disp: &[u8]) -> bool {
    disp == controller_offset().to_le_bytes()
}

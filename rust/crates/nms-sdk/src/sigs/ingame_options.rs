//! The game's Options page widgets, for the in-game Mods menu.
//!
//! Prologue bytes read from the exe (each unique on this build). The two page-stack entries are the
//! nmspy patterns whose RIP displacement is decoded at runtime, never a fixed address.

use crate::{CodePatch, ModSigs, Sig, SigUse};

pub const FINALIZE_PAGE: &str = "48 89 5C 24 10 48 89 6C 24 18 56 57 41 55 41 56 41 57 48 83 EC 40 48 8B 81 B8 27 00 00";
pub const END_PAGE: &str = "48 8B C4 55 57 48 8D 68 A1 48 81 EC A8 00 00 00 83 79 08 01";
pub const HEADER: &str = "48 89 5C 24 10 57 48 83 EC 40 83 79 08 01 48 8B DA 49 63 F8";
pub const BUTTON: &str = "48 89 5C 24 18 48 89 6C 24 20 56 57 41 57 48 81 EC 80 00 00 00";
pub const BOOLEAN: &str = "4C 89 44 24 18 56 57 41 55 41 57 48 83 EC 78 83 79 08 01 41 0F B6 F9";
pub const INT: &str = "44 89 4C 24 20 4C 89 44 24 18 55 56 41 57 48 8D 6C 24 D9 48 81 EC E0 00 00 00";
pub const FLOAT: &str = "F3 0F 11 5C 24 20 55 53 41 54 41 55 48 8D 6C 24 C8 48 81 EC 38 01 00 00";
pub const BEGIN_LIST: &str = "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 41 56 41 57 48 83 EC 20 33 DB 48 8D 79 2C";
pub const LIST_OPTION: &str = "40 53 48 83 EC 20 48 63 41 20 48 8D 59 20 4C 8B CA 83 F8 3F";
pub const END_LIST: &str = "48 8B C4 56 41 57 48 81 EC 98 02 00 00 44 8B 41 20 48 8D 71 20";
pub const REQUEST_OPTIONS_PAGE: &str = "48 89 5C 24 08 48 89 74 24 20 57 48 81 EC 00 01 00 00 8B D9";

/// The function that sets the page title text (OPTIONS, NETWORK, ...) from the page id. It skips ids
/// above 7, so our page keeps the default title until we set it. The root layer object it searches
/// is the pointer in the `mov rcx, [rip+disp32]` at byte `TITLE_ROOT_AT`.
pub const PAGE_TITLE: &str = "40 53 48 83 EC 40 66 0F 6F 0D ? ? ? ? 48 BB 69 2D 38 EB 08 EA DF 9D";
pub const TITLE_ROOT_AT: usize = 0xCD;

/// `cGcFrontendPageOptions::DoGameSwitcher`: draws the pause-menu Options list (nmspy pattern).
pub const GAME_SWITCHER: &str = "48 8B C4 55 53 56 57 41 54 41 55 41 56 41 57 48 8D A8 ? ? ? ? 48 81 EC ? ? ? ? 66 0F 6F 0D";
/// Finds a child element by name: `(element, &name16, recursive) -> element`. The page pass uses it
/// for `WHITE`. The call at the end is relative, so the pattern stops before it.
pub const FIND_CHILD: &str = "48 89 5C 24 08 57 48 83 EC 40 48 8B 99 50 01 00 00 41 0F B6 F8 45 33 C0 48 8D 4C 24 20 E8";
/// `cGcFrontendPage::Confirm` (`0x140901CF0`): hover and click test for one element (same function the
/// shop uses). `cmp byte [rcx+0x14C84], 0` is the page's input byte (`0x14C44` before build 183791).
pub const CONFIRM: &str = "48 89 6C 24 20 41 54 41 56 41 57 48 83 EC 40 80 B9 84 4C 01 00 00";
/// Inside `DoGameSwitcher`, at `BUSY_AT`: `mov rcx, [rip+disp32]`; at `BUSY_CHECK_AT`, two
/// instructions later: `cmp byte [rcx+0x87fd5a], 0` (`0x8594c2` before build 183791). The page skips
/// its click tests while that byte is set (a popup is open); so do we.
pub const BUSY_AT: usize = 0xCCF;
pub const BUSY_CHECK_AT: usize = 0xCEE;
pub const BUSY_CODE: [u8; 3] = [0x48, 0x8B, 0x0D];
pub const BUSY_CHECK: [u8; 7] = [0x80, 0xB9, 0x5A, 0xFD, 0x87, 0x00, 0x00];
pub const BUSY_OFFSET: usize = 0x87FD5A;
/// Offset of the pause page's element table, the first argument of the name lookup.
pub const TABLE_OFFSET: usize = 0x14478;

/// The game's text-entry popup (`cGcFrontendTextInput`, the box that renames a storage container).
pub const TEXT_START: &str = "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 48 81 EC 90 00 00 00 33 DB 44 88 89 F8 0A 00 00";
pub const TEXT_RESULT: &str = "48 89 5C 24 10 48 89 74 24 18 57 48 81 EC 80 00 00 00 8B 81 70 06 00 00 33 DB";
pub const TEXT_STOP: &str = "40 53 48 83 EC 30 33 DB 89 99 70 06 00 00 F3 0F 10 05 ? ? ? ? F3 0F 11 81 00 0B 00 00";
pub const TEXT_UPDATE: &str = "48 8B C4 55 57 48 8D A8 38 FE FF FF 48 81 EC B8 02 00 00 83 B9 70 06 00 00 01";
/// Inside `TEXT_UPDATE`, at this byte and `TEXT_CLOSE_LEN` long: the code that gives the keyboard back
/// (`mov rax,[rip+app]`, `mov r8b,1`, `mov edx,r15d` (r15d is 1), `mov rcx,[rax+<controller>]`,
/// `mov [rcx+0x40],r13b` (r13 is 0), `add rcx,0x1770`, `call set_actions`). Build 184271: at `0xB4D`,
/// controller `app+0x95AFB8`; before it the block sat at `0x96D` with `mov edx,1` and r12b (38 bytes).
pub const TEXT_CLOSE_AT: usize = 0xB4D;
pub const TEXT_CLOSE_LEN: usize = 36;
/// The input controller's action-set group, the `add rcx, imm32` at byte 26 of the close code
/// (`0x1500` before build 183791). Both the close-code check and the action-set calls use it.
/// Byte offsets inside the close code: controller disp32, the action-set imm32, the call rel32.
pub const TEXT_CLOSE_CONTROLLER_AT: usize = 16;
pub const TEXT_CLOSE_SETS_AT: usize = 27;
pub const TEXT_CLOSE_CALL_AT: usize = 31;
pub const CONTROLLER_SETS: u32 = 0x1770;

/// The Options "go back" step (`bool` in `cl`): pops one page off the page stack. The press that closes
/// the text box with B also reached this and left the page, so it is refused while our box is open.
/// `0x1406F6470` on build 184271; reads the app field `+0x89BEA8` (`0x89BD98` on 183791, `0x875498`
/// before).
pub const OPTIONS_BACK: &str = "48 89 5C 24 08 57 48 83 EC 20 48 8B 1D ? ? ? ? 0F B6 F9 8B 83 A8 BE 89 00 85 C0 74 0E";

/// `cmp dword [rip+disp32], imm8`: the page stack count. Displacement at byte 2, instruction 7 long.
pub const STACK_COUNT: &str = "83 3D ? ? ? ? ? 0F 84 ? ? ? ? 48 89 9C 24 ? ? ? ? 48 8D 4C 24";
/// `mov rax, [rip+disp32]`: the page stack data pointer. Displacement at byte 3, instruction 7 long.
pub const STACK_DATA: &str = "48 8B 05 ? ? ? ? FF C9 48 8B 9D";

pub const TABLE: ModSigs = ModSigs {
    owner: "InGameMenu",
    sigs: &[
        Sig { name: "FINALIZE_PAGE", pattern: FINALIZE_PAGE, kind: SigUse::Hook },
        Sig { name: "END_PAGE", pattern: END_PAGE, kind: SigUse::Hook },
        Sig { name: "PAGE_TITLE", pattern: PAGE_TITLE, kind: SigUse::Hook },
        Sig { name: "GAME_SWITCHER", pattern: GAME_SWITCHER, kind: SigUse::Hook },
        Sig { name: "FIND_CHILD", pattern: FIND_CHILD, kind: SigUse::Locate },
        Sig { name: "CONFIRM", pattern: CONFIRM, kind: SigUse::Locate },
        Sig { name: "TEXT_START", pattern: TEXT_START, kind: SigUse::Locate },
        Sig { name: "TEXT_RESULT", pattern: TEXT_RESULT, kind: SigUse::Locate },
        Sig { name: "TEXT_STOP", pattern: TEXT_STOP, kind: SigUse::Locate },
        Sig { name: "TEXT_UPDATE", pattern: TEXT_UPDATE, kind: SigUse::Hook },
        Sig { name: "OPTIONS_BACK", pattern: OPTIONS_BACK, kind: SigUse::Hook },
        Sig { name: "HEADER", pattern: HEADER, kind: SigUse::Locate },
        Sig { name: "BUTTON", pattern: BUTTON, kind: SigUse::Locate },
        Sig { name: "BOOLEAN", pattern: BOOLEAN, kind: SigUse::Locate },
        Sig { name: "INT", pattern: INT, kind: SigUse::Locate },
        Sig { name: "FLOAT", pattern: FLOAT, kind: SigUse::Locate },
        Sig { name: "BEGIN_LIST", pattern: BEGIN_LIST, kind: SigUse::Locate },
        Sig { name: "LIST_OPTION", pattern: LIST_OPTION, kind: SigUse::Locate },
        Sig { name: "END_LIST", pattern: END_LIST, kind: SigUse::Locate },
        Sig { name: "REQUEST_OPTIONS_PAGE", pattern: REQUEST_OPTIONS_PAGE, kind: SigUse::Locate },
        Sig { name: "STACK_COUNT", pattern: STACK_COUNT, kind: SigUse::Locate },
        Sig { name: "STACK_DATA", pattern: STACK_DATA, kind: SigUse::Locate },
    ],
    patches: &[] as &[CodePatch],
};

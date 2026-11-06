//! PickerTweaks: search in the item pickers, type an amount.
//!
//! The text popup routines (`TEXT_START`, `TEXT_RESULT`, `TEXT_STOP`, `TEXT_UPDATE`) are the in-game
//! Mods page's (`ingame_options`), reused as they are; `TEXT_UPDATE` is the shared `text_input_update` hook.
//!
//! Every address below was re-read in the disassembly of build 183791 (2026-10-28). Struct offsets
//! that moved since build 181442 are noted where they are used.

use crate::{ModSigs, Sig, SigUse};

/// `0x1406D62B0(popup, element, state, out) -> u32`: the picker card predicate. 0 show, 1 show with
/// substitute data, 2 hide (card, page count and scroll dots). Four register arguments, no stack ones.
pub const PREDICATE: &str = "48 89 5C 24 10 48 89 74 24 18 57 48 83 EC 50 49 8B F9 49 8B F0 48 8B DA 4C 8B D1 4D 85 C0 75 ?";
/// `0x1409000B0(page, dt: f32)`: the held-stack (stack split) input tick. The only caller
/// (`0x14090BEB6`) passes `rcx` and `xmm1` and nothing else. `page+0x196F8` is its "stack held" byte
/// (`0x19698` before build 183791).
pub const HELD_TICK: &str = "48 8B C4 55 48 81 EC B0 00 00 00 80 B9 F8 96 01 00 00 48 8B E9";
/// `0x14081F1A0(this, page)`: the refiner page. Hooked at the prologue only (chunked function).
pub const REFINER_PAGE: &str = "48 89 54 24 10 48 89 4C 24 08 55 53 41 55 41 57 48 8D AC 24 78 FC FF FF";
/// In `REFINER_PAGE`: `mov rcx, rdx` then `call <machine getter>` (`0x14081EAE0(page) -> unit`).
pub const GETTER_MOV_AT: usize = 0x25;
pub const GETTER_MOV: [u8; 3] = [0x48, 0x8B, 0xCA];
pub const GETTER_CALL_AT: usize = 0x28;
/// `0x1410D7B90(unit) -> u32`: the lower bound the page gives the amount arrows (count of the
/// input already in the machine; read only).
pub const REFINER_MIN: &str = "48 89 5C 24 10 48 89 74 24 18 57 48 83 EC 20 48 8B 81 60 05 00 00 33 F6 4C 8B D9";
/// `0x1410D7CA0(unit) -> u32`: added to `REFINER_MIN` for the upper bound of the arrows.
pub const REFINER_MAX: &str = "40 53 48 83 EC 20 48 8D 54 24 38 C7 44 24 38 00 00 00 00";
/// `0x1410D7D50(unit, u16 amount)`: the game's own amount setter (clamps, repacks `unit+0x2A0`).
pub const REFINER_SETTER: &str = "40 53 56 48 83 EC 28 48 89 7C 24 50 33 DB 0F B7 FA";
/// `0x140EE4A40(app+0x60, id) -> record`: product lookup (pure hash find).
pub const PRODUCT_LOOKUP: &str = "48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 20 48 8B F9 48 8B EA 48 81 C1 C0 03 00 00";
/// `0x140ED6E10(app+0x60, id) -> record`: substance lookup (pure hash find).
pub const SUBSTANCE_LOOKUP: &str = "48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 20 48 8B F9 48 8B EA 48 81 C1 00 03 00 00";
/// `0x142C40F80(table, key, 0) -> utf8`: localisation lookup. Copies the key to its own stack,
/// upper-cases it, finds it in the table; returns the key pointer itself when the id is unknown.
pub const LOC_LOOKUP: &str = "48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 40 0F 57 C0";
/// `0x1401CEC30() -> table`: the localisation table (a function-local static, built at boot). The
/// thread-local offset in it is `0x1D48` on build 184271 (`0x1B28` before).
pub const LOC_TABLE: &str = "48 83 EC 28 65 48 8B 04 25 58 00 00 00 B9 48 1D 00 00 48 8B 00 8B 04 01 39 05 ? ? ? ? 0F 8F ? ? ? ? \
48 8D 05 ? ? ? ? 48 83 C4 28 C3 4C 89 00 4C 89 40 08 4C 89 40 10 48 8D 40 40";

/// `0x140EE3C60(app+0x60, id, bool make) -> record`: technology lookup. Called with `make = 0` only:
/// a pure hash find plus a list walk (with 1 it builds a missing procedural technology). The name
/// key the technology card draws is at record `+0x50` (`0x1406A490C`).
pub const TECH_LOOKUP: &str = "48 89 5C 24 10 48 89 6C 24 18 48 89 74 24 20 57 48 83 EC 30 48 8B F1 41 0F B6 E8 48 81 C1 40 03 00 00";

/// `0x1409172D0(host)`: the frontend page host, once per frame while a menu page is up. Located,
/// never hooked: the input shield reads three things out of it.
/// Ends in `add rcx, 0x1EF60` on build 184271 (`0x1EE50` on 183791, `0x1EDD0` before).
pub const PAGE_HOST: &str = "48 8B C4 55 53 56 57 41 54 48 8D 68 B8 48 81 EC 20 01 00 00 4C 89 68 D0 48 8B F9 4C 89 70 C8 48 81 C1 60 EF 01 00";
/// In `PAGE_HOST`: `call <frontend input builder>` (`0x140204590`), which fills the frame's
/// button states from the action table.
pub const HOST_BUILD_CALL_AT: usize = 0x336;
/// In `PAGE_HOST`: `mov [rip+G], r12`, the pointer every page reads the frame's input state through.
pub const HOST_STATE_STORE_AT: usize = 0x371;
pub const HOST_STATE_STORE: [u8; 3] = [0x4C, 0x89, 0x25];
/// In `PAGE_HOST`: `lea rcx, [rdi+0x8E8]; call TEXT_UPDATE` (`+0x483` and `rdi+0x8E0` before build
/// 183791). Text `Update` runs after the input builder and before the page and its popup in the same
/// frame.
pub const HOST_UPDATE_CALL_AT: usize = 0x480;
pub const HOST_UPDATE_CALL: [u8; 7] = [0x48, 0x8D, 0x8F, 0xE8, 0x08, 0x00, 0x00];
/// In the input builder: `mov dword [rbx+0x34], 2` (Fe_Select pressed) and `mov [rbx+0x50], r15d`
/// (the last button state written): the button states are the 8 dwords at `+0x34..+0x54`
/// (`+0x131C` and `+0x15F7` on 183791 and 183877, `+0x130E` and `+0x15E9` before).
pub const BUILD_SELECT_AT: usize = 0x1350;
pub const BUILD_SELECT: [u8; 7] = [0xC7, 0x43, 0x34, 0x02, 0x00, 0x00, 0x00];
pub const BUILD_LAST_AT: usize = 0x162B;
pub const BUILD_LAST: [u8; 4] = [0x44, 0x89, 0x7B, 0x50];

/// `0x142CA9BC0(device, f32 x, f32 y)`: the mouse device's own "put the cursor here" (device vtable
/// `0x1435CF9A0` `+0x38`; the device is the input controller `+0x10`). Rounds both values, writes
/// position and previous position alike (`movups [rbx+0x2E0]` at `+0x32`, `movups [rbx+0x2F0]` at
/// `+0x52`; `0x200`/`0x210` before build 183791, so the next poll sees no mouse motion and the game stays in its input mode), then calls
/// `glfwSetCursorPos` (`+0x6B`). Does nothing while `[device+0x2C]` is set. Never hooked.
pub const DEVICE_SET_POS: &str = "40 53 48 83 EC 40 80 79 2C 00 48 8B D9 0F 29 74 24 30 0F 28 F2 75 5E 0F 28 C1 \
0F 29 7C 24 20 E8 ? ? ? ? 0F 57 FF F3 0F 5A F8 0F 28 C6";
pub const DEV_SET_X_AT: usize = 0x32;
pub const DEV_SET_X: [u8; 7] = [0x0F, 0x11, 0xBB, 0xE0, 0x02, 0x00, 0x00];
pub const DEV_SET_Y_AT: usize = 0x52;
pub const DEV_SET_Y: [u8; 7] = [0x0F, 0x11, 0xB3, 0xF0, 0x02, 0x00, 0x00];
pub const DEV_SET_GLFW_CALL_AT: usize = 0x6B;
/// In `DEVICE_SET_POS`: `call <platform accessor>` (`0x1401D12F0`) and `mov rcx, [rax+0x1E0]` (the
/// GLFW window), read for the log (cursor mode `window+0x78`).
pub const DEV_SET_PLATFORM_CALL_AT: usize = 0x59;
pub const DEV_SET_WINDOW_AT: usize = 0x64;
pub const DEV_SET_WINDOW: [u8; 7] = [0x48, 0x8B, 0x88, 0xE0, 0x01, 0x00, 0x00];
/// GLFW 3.3 `glfwSetCursorPos` (`0x1419569F0`, the target of `DEV_SET_GLFW_CALL_AT`): prologue,
/// `cmp [rbx+0x358], rax` (only the active window), `cmp dword [rbx+0x78], 0x34003` (disabled mode
/// writes GLFW's virtual position only, no OS cursor move).
pub const SET_POS_HEAD: [u8; 8] = [0x40, 0x53, 0x48, 0x83, 0xEC, 0x40, 0x83, 0x3D];
pub const SET_POS_HANDLE_AT: usize = 0x7B;
pub const SET_POS_HANDLE: [u8; 7] = [0x48, 0x39, 0x83, 0x58, 0x03, 0x00, 0x00];
pub const SET_POS_MODE_AT: usize = 0x84;
pub const SET_POS_MODE: [u8; 7] = [0x81, 0x7B, 0x78, 0x03, 0x40, 0x03, 0x00];
/// In `glfwSetCursorPos`, disabled mode: `movsd [rbx+0x1E8], xmm6; movsd [rbx+0x1F0], xmm7` (GLFW's
/// virtual cursor, the position the game reads in disabled mode).
pub const SET_POS_VIRTUAL_AT: usize = 0x8D;
pub const SET_POS_VIRTUAL: [u8; 16] =
    [0xF2, 0x0F, 0x11, 0xB3, 0xE8, 0x01, 0x00, 0x00, 0xF2, 0x0F, 0x11, 0xBB, 0xF0, 0x01, 0x00, 0x00];
/// In `glfwSetCursorPos`, other modes: `jmp _glfwPlatformSetCursorPos` (`0x14195B980`).
pub const SET_POS_PLATFORM_JMP_AT: usize = 0xC5;
/// `_glfwPlatformSetCursorPos(window, f64 x, f64 y)`: stores `lastCursorPos` (`+0x388/+0x38C`,
/// at `+0x2C`), `ClientToScreen`, `SetCursorPos`. No active-window check (unlike `glfwSetCursorPos`).
pub const PLATFORM_SET_HEAD: [u8; 18] =
    [0x40, 0x53, 0x48, 0x83, 0xEC, 0x20, 0xF2, 0x0F, 0x2C, 0xC1, 0x48, 0x8B, 0xD9, 0xB9, 0x00, 0x10, 0x00, 0x00];
pub const PLATFORM_SET_LAST_AT: usize = 0x2C;
pub const PLATFORM_SET_LAST: [u8; 6] = [0x89, 0x83, 0x88, 0x03, 0x00, 0x00];
/// Build 181442 (`0x1402020E6`), in the frontend input builder: `cmp [App+0x85A464], sil` (text
/// manager `+0xAF4`, in-game keyboard on) and `cmp dword [App+0x859FE0], 0` (manager state),
/// `setne r15b`. When that "text box up" bit changed the builder took the mouse device position as the
/// cursor that frame: the jump on box open and close. At `+0x222` the cursor store
/// `movss [rbx+0xC], xmm11; movss [rbx+0x10], xmm12` (frontend cursor in the page state).
///
/// Build 183791 removed this check from the builder (`0x140205BCC` goes straight on to the cursor
/// code; the text manager, now `App+0x880208`, is not read there any more). The pattern is kept as it
/// was and is expected to be missing: `cursor::locate` then fails and the cursor write-back stays off,
/// which is what a build without the snap needs. Not re-pointed at anything.
pub const FRONTEND_TEXT_SNAP: &str = "40 38 B1 64 A4 85 00 74 0D 83 B9 E0 9F 85 00 00 41 0F 95 C7";
pub const SNAP_CURSOR_STORE_AT: usize = 0x222;
pub const SNAP_CURSOR_STORE: [u8; 12] = [0xF3, 0x44, 0x0F, 0x11, 0x5B, 0x0C, 0xF3, 0x44, 0x0F, 0x11, 0x63, 0x10];
/// `0x142C98C80(controller, ctxIndex) -> bool`: input controller vtable `+0x30`, "keyboard and mouse
/// active" (`[ctl + 0x5B0 + idx*0x470 + 0xA2]`, `ctx+0xA2`); false means the game is in pad mode.
/// Located for the byte check only; PickerTweaks reads the byte (never writes it). Contexts were
/// `0x4C0 + idx*0x410` before build 183791; the stride and the `0x652` keep the pattern unique (its
/// siblings read `ctx+0xA3` and `ctx+0xA5`).
pub const KBM_QUERY: &str = "83 FA FF 75 08 8B 51 18 83 FA 04 74 05 83 FA 05 76 03 32 C0 C3 48 63 C2 48 69 C0 70 04 00 00 \
0F B6 84 08 52 06 00 00 C3";

/// `0x1406D8F90(page, layer)`: the inventory craft list (popup kind 2/3), run every frame while it is
/// up. It rebuilds its temporary inventories each frame, one `CRAFT_ADD` call per recipe, at these
/// three sites (each `mov rcx,[rcx+rax*8]; call CRAFT_ADD`): technologies (`+0x295`, `+0x4BE`) and
/// products (`+0x7E9`), one byte later than on build 181442. PickerTweaks sends these three calls
/// to its own filter.
pub const CRAFT_LIST: &str = "44 89 44 24 18 48 89 54 24 10 55 56 57 41 55 41 57 48 8D AC 24 10 FD FF FF 48 81 EC F0 03 00 00 \
33 C0 0F 57 C0 0F 11 44 24 68 4C 8B EA C7 44 24 68 49 4E 56 00";
pub const CRAFT_ADD_SITES: [usize; 3] = [0x295, 0x4BE, 0x7E9];
/// The 4 bytes before each site call: `mov rcx, [rcx+rax*8]`.
pub const CRAFT_SITE_PREFIX: [u8; 4] = [0x48, 0x8B, 0x0C, 0xC1];
/// `0x1404D9590(inventory, out, element) -> out`: add one element (id `+0`, type `+0x24`, position
/// `-1,-1` = first free slot) to an inventory. Writes the slot or `-1` (not added, `0x1404D998C`) to
/// `*out` and returns `out`. Three register arguments; `r9` is scratch.
pub const CRAFT_ADD: &str = "48 89 5C 24 10 48 89 74 24 18 55 57 41 56 48 8D 6C 24 B0 48 81 EC 50 01 00 00 41 0F 10 00 \
4C 8B F2 48 8B F9 41 0F 10 48 10";
/// In `CRAFT_ADD`: the result stores, both 8 bytes wide: `mov [r14], rax` (placed, `+0x3F7`) and
/// `mov qword [r14], -1` (not placed, `+0x3FC`; imm32 sign-extended to 64 bits).
pub const CRAFT_ADD_OK_AT: usize = 0x3F7;
pub const CRAFT_ADD_OK: [u8; 3] = [0x49, 0x89, 0x06];
pub const CRAFT_ADD_FAIL_AT: usize = 0x3FC;
pub const CRAFT_ADD_FAIL: [u8; 7] = [0x49, 0xC7, 0x06, 0xFF, 0xFF, 0xFF, 0xFF];

pub const TABLE: ModSigs = ModSigs {
    owner: "PickerTweaks",
    sigs: &[
        Sig { name: "PREDICATE", pattern: PREDICATE, kind: SigUse::Hook },
        Sig { name: "HELD_TICK", pattern: HELD_TICK, kind: SigUse::Hook },
        Sig { name: "REFINER_PAGE", pattern: REFINER_PAGE, kind: SigUse::Hook },
        Sig { name: "REFINER_MIN", pattern: REFINER_MIN, kind: SigUse::Locate },
        Sig { name: "REFINER_MAX", pattern: REFINER_MAX, kind: SigUse::Locate },
        Sig { name: "REFINER_SETTER", pattern: REFINER_SETTER, kind: SigUse::Locate },
        Sig { name: "PRODUCT_LOOKUP", pattern: PRODUCT_LOOKUP, kind: SigUse::Locate },
        Sig { name: "SUBSTANCE_LOOKUP", pattern: SUBSTANCE_LOOKUP, kind: SigUse::Locate },
        Sig { name: "LOC_LOOKUP", pattern: LOC_LOOKUP, kind: SigUse::Locate },
        Sig { name: "LOC_TABLE", pattern: LOC_TABLE, kind: SigUse::Locate },
        Sig { name: "TECH_LOOKUP", pattern: TECH_LOOKUP, kind: SigUse::Locate },
        Sig { name: "PAGE_HOST", pattern: PAGE_HOST, kind: SigUse::Locate },
        Sig { name: "DEVICE_SET_POS", pattern: DEVICE_SET_POS, kind: SigUse::Locate },
        Sig { name: "FRONTEND_TEXT_SNAP", pattern: FRONTEND_TEXT_SNAP, kind: SigUse::Locate },
        Sig { name: "KBM_QUERY", pattern: KBM_QUERY, kind: SigUse::Locate },
        Sig { name: "CRAFT_LIST", pattern: CRAFT_LIST, kind: SigUse::Locate },
        Sig { name: "CRAFT_ADD", pattern: CRAFT_ADD, kind: SigUse::Locate },
    ],
    patches: &[],
};

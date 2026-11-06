//! RefinerGuard: refuse to move or delete a refiner that still holds items.
//!
//! Every address below was re-read in the disassembly of build 183791 (89,491,528 bytes, 2026-10-28), except
//! where a line says otherwise.

use crate::{ModSigs, Sig, SigUse};

/// `RemovePart(tool, handle: u32, mode: i32) -> bool` `0x1403D53E0` (build 184271): every Move and Delete
/// of a base or corvette part ends here. Three register arguments (`rcx`, `edx`, `r8d`), no stack arguments
/// (only the shadow homes above the return address; re-read on 184271, where the frame shrank by 0x10).
/// Returns 1 on success, 0 when refused (the vanilla refusals return 0 before anything is changed). The
/// frame sizes are wildcards; 1 hit on 183877 and 184271.
pub const REMOVE_PART: &str = "48 89 5C 24 10 44 89 44 24 18 48 89 4C 24 08 55 56 57 41 54 41 55 41 56 41 57 48 8D 6C 24 ? 48 81 EC ? ? ? ? 44 0F 29 9C 24 ? ? ? ? 8B DA 48 8B F1 45 33 FF E8";

/// Inside `RemovePart` (`0x1403D5A5B`): `xor eax,eax; lea rdx,[rsp+0x50]; mov ecx,ebx; mov [rsp+0x50],rax;
/// call MaintLookup; mov rcx,[rsp+0x50]; test rcx,rcx; je; mov rax,[rcx]; call [rax+0xE0]` (the local was
/// `[rbp-0x40]` before build 184271). The call is the lookup the game itself uses to find the machine it
/// then clears (slot 28). 1 hit.
pub const MAINT_SITE: &str =
    "33 C0 48 8D 54 24 ? 8B CB 48 89 44 24 ? E8 ? ? ? ? 48 8B 4C 24 ? 48 85 C9 74 09 48 8B 01 FF 90 E0 00 00 00";
/// Offset of the `E8` of the lookup call inside `MAINT_SITE`.
pub const MAINT_SITE_CALL: usize = 14;

/// First bytes of `MaintLookup(handle: u32, out: *mut comp) -> u32` `0x1403E3300` (a template instance,
/// the prologue alone hits many times, so it is reached through `MAINT_SITE` and only checked here). Pure:
/// handle table reads (component table `0x141870E40`; the other table and hash-find callees were read in
/// build 180836 and the body is byte-identical since), the type getter below; only the out pointer and the
/// stack are written.
pub const MAINT_LOOKUP_HEAD: [u8; 23] = [
    0x48, 0x89, 0x5C, 0x24, 0x10, 0x55, 0x56, 0x57, 0x48, 0x83, 0xEC, 0x30, 0x48, 0x8B, 0xF2, 0x8B, 0xF9, 0xF7, 0xC1,
    0x00, 0x00, 0xF8, 0xFF,
];
/// Offset of the `E8` to the maintenance type getter inside `MaintLookup`.
pub const MAINT_LOOKUP_GETTER_CALL: usize = 0x3F;
/// First bytes of the maintenance type getter `0x141048CE0(table) -> comp`: tries seven component types
/// (`movsx ecx, word [rip+disp]; mov rdx,rbx; call 0x142D95FC0`), the third is the refiner's.
pub const MAINT_GETTER_HEAD: [u8; 12] = [0x40, 0x53, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0xD9, 0x0F, 0xBF, 0x0D];
/// Bytes of the getter searched for its type words.
pub const MAINT_GETTER_SPAN: usize = 0x98;

/// `0x142D95FC0(type: i32, table) -> component`. Two register arguments; reads the table bitset and calls
/// the rank function (no stores except to its stack; body identical to the 180836 one). 1 hit.
pub const COMP_GET: &str = "40 53 48 83 EC 20 4C 63 C9 48 8B DA 48 85 D2 74 49";

/// In the refiner update (`0x1410D9FC1`): `mov rcx,[rip+app]; mov rdx,rdi; add rcx,0x87F920; call ..;
/// movsx ecx, word [rip+refiner type]; mov rdx,[rdi+8]`. Gives the application global and the refiner
/// component type word (`[unit+8]` is the component table). The application sub-object moved from
/// `0x859090` to `0x87F920` in build 183791. 1 hit.
pub const REFINER_ANCHOR: &str =
    "48 8B 0D ? ? ? ? 48 8B D7 48 81 C1 20 F9 87 00 E8 ? ? ? ? 0F BF 0D ? ? ? ? 48 8B 57 08";
/// Offset of the application global's `disp32` (instruction ends 4 bytes later).
pub const ANCHOR_APP_DISP: usize = 3;
/// Offset of the refiner type word's `disp32` (instruction ends 4 bytes later).
pub const ANCHOR_TYPE_DISP: usize = 25;

/// The game's own refusal notice for a running refiner, inside `RefinerRefund` `0x1410D7FB0` at
/// `0x1410D7FDE`:`mov rdi,[rip+app]` .. translate `REFINER_CANNOT_PICK_UP` .. `movss xmm2,[rip+1.0]`
/// .. `lea r9,[rip+colour]` .. `lea rcx,[rdi+0x86E3D0]; mov [rsp+0x20], audio; call AddTimedMessage;
/// lea rcx,[rsp+0xD0]; call release`. That is the whole vanilla refusal: an on-screen message (not modal)
/// and `return 0`. RefinerGuard repeats exactly this call with its own text. 1 hit.
pub const REFUSAL_NOTICE: &str = "48 8B 3D ? ? ? ? 49 8D 4B 08 45 89 73 08 0F B6 98 54 04 00 00 E8 ? ? ? ? E8 ? ? ? ? 48 8D 0D ? ? ? ? 84 DB 48 8D 15 ? ? ? ? 48 0F 45 D1 45 33 C0 48 8B C8 E8 ? ? ? ? F3 0F 10 15 ? ? ? ? 48 8D 8C 24 D0 00 00 00 44 88 74 24 50 4C 8D 0D ? ? ? ? 44 88 74 24 48 0F 57 C0 44 88 74 24 40 48 8B D0 F3 0F 11 44 24 38 48 89 4C 24 28 48 8D 8F ? ? ? ? C7 44 24 20 ? ? ? ? E8 ? ? ? ? 48 8D 8C 24 D0 00 00 00 E8 ? ? ? ?";
/// `mov rdi,[rip+app]`: offset of the `disp32` (instruction ends 4 bytes later).
pub const NOTICE_APP_DISP: usize = 3;
/// `movss xmm2,[rip+duration]`: offset of the `disp32` (instruction ends 4 bytes later).
pub const NOTICE_DURATION_DISP: usize = 0x43;
/// `lea r9,[rip+colour]`: offset of the `disp32` (instruction ends 4 bytes later).
pub const NOTICE_COLOUR_DISP: usize = 0x57;
/// `lea rcx,[rdi+object]`: offset of the `disp32` (the message list inside the application object).
pub const NOTICE_OBJECT_DISP: usize = 0x79;
/// `mov dword [rsp+0x20], audio`: offset of the `imm32`.
pub const NOTICE_AUDIO_IMM: usize = 0x81;
/// `call AddTimedMessage` (`0x1409D3FA0`) and `call release` (`0x142DD6870`): offsets of the `E8`.
pub const NOTICE_ADD_CALL: usize = 0x85;
pub const NOTICE_RELEASE_CALL: usize = 0x92;

/// Build tool command 9 (Move) at `0x1403C5760`: `mov edx,[rdi+0x7C8]; xor r8d,r8d; mov rcx,rdi;
/// call RemovePart; test al,al; je skip; mov [rdi+0xB5BC],2; mov [rdi+0x424],2`. On 0 the tool writes
/// nothing (it stays in its menu, as for the vanilla refusals). 1 hit.
pub const MOVE_SITE: &str = "8B 97 C8 07 00 00 45 33 C0 48 8B CF E8 ? ? ? ? 84 C0 0F 84 ? ? ? ? C7 87 BC B5 00 00 02 00 00 00 C7 87 24 04 00 00 02 00 00 00";
/// Offset of the `E8` to `RemovePart` inside `MOVE_SITE`; the return address is 5 bytes later.
pub const MOVE_SITE_CALL: usize = 12;

/// `DeleteSelected(tool)` `0x1403D45F0` (build tool command 2): deletes the queued handle (mode 1), else
/// tail-jumps to `RemovePart(tool, [tool+0x7C8], 0)`, so `RemovePart` returns straight into the command
/// loop, which ignores the result. 1 hit, 1 `E8` caller (the command loop, `0x1403C55B0`).
pub const DELETE_SELECTED: &str = "40 57 48 83 EC 20 8B 81 BC B8 00 00 48 8B F9 A9 00 00 F8 FF";
/// In `DeleteSelected` at this offset: `mov edx,[rdi+0x7C8]; xor r8d,r8d; mov rcx,rdi; add rsp,0x20;
/// pop rdi; jmp RemovePart` (the `E9` is the last byte here).
pub const DELETE_TAIL_AT: usize = 0x5C;
pub const DELETE_TAIL: [u8; 18] =
    [0x8B, 0x97, 0xC8, 0x07, 0x00, 0x00, 0x45, 0x33, 0xC0, 0x48, 0x8B, 0xCF, 0x48, 0x83, 0xC4, 0x20, 0x5F, 0xE9];
/// The command loop's call to `DeleteSelected` lies this many bytes or fewer before `MOVE_SITE`
/// (`0x1403C55B0` versus `0x1403C5760`); exactly one `E8` to it must be found there.
pub const DELETE_CALL_WINDOW: usize = 0x400;

pub const TABLE: ModSigs = ModSigs {
    owner: "RefinerGuard",
    sigs: &[
        Sig { name: "REMOVE_PART", pattern: REMOVE_PART, kind: SigUse::Hook },
        Sig { name: "MAINT_SITE", pattern: MAINT_SITE, kind: SigUse::Locate },
        Sig { name: "COMP_GET", pattern: COMP_GET, kind: SigUse::Locate },
        Sig { name: "REFINER_ANCHOR", pattern: REFINER_ANCHOR, kind: SigUse::Locate },
        Sig { name: "REFUSAL_NOTICE", pattern: REFUSAL_NOTICE, kind: SigUse::Locate },
        Sig { name: "MOVE_SITE", pattern: MOVE_SITE, kind: SigUse::Locate },
        Sig { name: "DELETE_SELECTED", pattern: DELETE_SELECTED, kind: SigUse::Locate },
    ],
    patches: &[],
};

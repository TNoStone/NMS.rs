# How NMS.rs works

## Loading

`NMS.exe` imports `XINPUT9_1_0.dll`. NMS.rs ships a DLL with that name in `Binaries\`
(`crates/nms-proxy`). The four XInput functions pass straight through to the real System32 DLL,
so controllers work as before.

Only one file can be called `xinput9_1_0.dll`, and other mods ship one too. So the same loader is
also built as `winmm.dll` (imported by `NMS.exe`) and `version.dll` (imported by the PlayFab, DLSS
and Streamline DLLs that `NMS.exe` loads at startup). Once loaded, every module that imports that
name gets our file, so these forward every export of the System32 DLL: one small stub per export
(`crates/nms-proxy/exports/*.txt`), which loads the real DLL by full path on its first call. The
manager tells our files apart from other mods' by a marker inside them, not by name.

`DllMain` does nothing but start one boot thread and return, so no work runs under the loader
lock. The boot thread:

1. stops at once if the host process is not `NMS.exe`, or if another NMS.rs file already booted
   in this process (two installed, say `xinput9_1_0.dll` and `winmm.dll`: the second only
   forwards and writes one line to `core.log`), or if `NMS.rs\disabled` exists (kill switch);
2. runs `NMS.rs\nms-rs.exe prepare` when the combined UI merge is out of date (see below);
3. takes a copy of the game's code section at load time, so later searches see the original
   bytes even after another mod has hooked a function;
4. registers every mod (`crates/nms-mods`), each of which finds its signatures, installs its
   hooks once and applies the patches its switches ask for;
5. adds the Mods page to the game's Options menu and opens the settings pipe.

A mod stays off, with status "Python copy present", while its version 3 `.py` file is still in its
folder, so the old and new loader never hook the same function.

## Signatures, patches and hooks

- No address is hard-coded. Every function or patch site is found by a byte pattern with
  wildcards, listed per mod in `crates/nms-sdk/src/sigs`. A pattern must match exactly once; no
  match or two matches means "not found", and that part of the mod stays off.
- Byte patches fail closed. A patch is written only when the bytes at the site are exactly the
  expected original. Once written, it is found again by its patched bytes, so the same call can
  undo it. Any other state logs "not changed" and leaves the game vanilla.
- Hooks are installed once. Enabling or disabling a detour while the game runs is not safe
  (other threads may be inside the prologue), so a switch is an atomic flag the detour checks. A
  detour declares every argument the game function takes, stack arguments included, and forwards
  them all. Code reachable from a detour never panics (`panic = "abort"` in release builds) and
  takes locks with `try_lock`.
- When two mods need the same function, it gets one detour through
  `nms_core::shared_hook!` (`crates/nms-mods/src/common/hooks.rs`) and each mod subscribes to it.
  Two detours on one prologue are never allowed.
- For an `E8`/`E9` call or jump, the rel32 displacement starts at the byte after the opcode and
  is relative to the end of the instruction.

`sigcheck` checks all of this offline against an `NMS.exe`: every signature unique, every patch
site holding the expected bytes, and no two mods hooking one function entry unless it is in the
shared list. `--compare OLD.exe NEW.exe` prints BROKE, FIXED, NEW and GONE lines and a verdict
per mod. Exit code 1 means something broke.

## Settings

Each mod has a list of settings (`SettingSpec`: key, label, kind) and keeps its values in
`GAMEDATA\MODS\<Mod>\<module>.json`. The in-game Mods page and the command line both go through
the same `GameMod::set`, which applies the change live and saves it. `nms-rs status` and
`nms-rs set <mod> <key> <value>` talk to the running game over a local named pipe (one JSON
object per line), so a settings window can never hold back the game's frame rate.

## UI merge

UI mods replace game files (MBIN). Two mods that replace the same file would overwrite each other,
so NMS.rs mods ship AMUMSS-style lua scripts that describe their changes, and the merge applies
every such script under `GAMEDATA\MODS` to one vanilla copy of each shared file:

1. find all scripts and the game files they touch;
2. unpack the vanilla files from the game's paks (HGPAKtool) and decompile them (MBINCompiler);
3. apply each script's change tables to the MXML (`crates/nms-amumss`), in folder order, with
   the names in `NMS.rs\merge_order.txt` last;
4. compile the result into `GAMEDATA\MODS\z_Combined Mods` and record what went in
   (`combined.txt`). Finished MBIN files two mods both replace without a script are listed in
   `NMS.rs\conflicts.txt`.

A hash of every input decides whether the merge must run again, so a normal start costs nothing.

## Sizes

`nms-rs sizes apply` (or Apply sizes in the manager) builds the BiggerItemGrids and
SmallerInventorySlots UI files from the installed game in the same way: unpack, decompile, edit
the MXML in Rust (`crates/nms-launcher/src/sizes`), compile. Values are clamped to the ranges the
game layout can take, and the pick is saved in `NMS.rs\sizes.json`. The loader then sets the
matching page and grid sizes in game code at start.

## Downloaded tools

MBINCompiler and HGPAKtool are not bundled. `crates/nms-launcher/src/tools.rs` pins one GitHub
release of each by tag and SHA-256; the manager downloads them only when they are missing and a
merge or Apply sizes needs them, checks the hash before anything is written, and installs nothing
on a mismatch.

## Files in a game folder

```
Binaries\xinput9_1_0.dll           the loader
NMS.rs\nms-rs.exe                  manager and CLI
NMS.rs\stock\<Mod>\                copies the manager installs from (stock.json has versions)
NMS.rs\parked\<Mod>\               mods you parked
NMS.rs\logs\core.log               loader log
NMS.rs\sizes.json, merge_order.txt, conflicts.txt, backup\
GAMEDATA\MODS\<Mod>\               installed mods, their settings (.json) and logs
GAMEDATA\MODS\z_Combined Mods\     merged UI files
```

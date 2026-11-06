# Working on NMS.rs

Windows only for now (the loader and most tests use Win32 APIs). Use the MSVC Rust toolchain.

## Build and test

From `rust\`:

```
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release -p nms-launcher -p nms-proxy
cargo run -q -p sigcheck --release [-- path\to\NMS.exe]
```

Some tests read a real game folder and skip when it is not there (signature checks against
`Binaries\NMS.exe`, other people's lua scripts). They find the game by walking up from the crate,
so a checkout at `<game>\Modding\NMS.rs` works as is; elsewhere set `NMS_GAME_DIR` to the game
folder. Tests marked `#[ignore]` need a game folder, the downloaded tools or old Python golden output;
run them with `cargo test -- --ignored` when you change the size builders or the tool versions.

To try a build in game: quit the game, then from inside the game folder run
`target\release\nms-rs.exe install` (copies the DLL into `Binaries` and creates `NMS.rs\`).
`nms-rs status` and `nms-rs set "<Mod>" <key> true|false` work while the game runs.

## Release

```
py -3 release.py rust-build --version <v>
py -3 release.py rust --version <v> --out <dir>
py -3 -I dev\test_release_rust.py
```

`rust-build` uses the `dist` profile and fails if a build path is still in the binaries. `rust`
collects `nms-rs.exe`, the DLL, `NMS.rs/stock/<Mod>/...` for every mod in `RUST_STOCK_MODS`
(the mod folder plus its default preset, without READMEs, `.py`, logs or user `.json`),
`stock.json` and a `SHA256SUMS.txt`, and writes `<zip>.sha256` next to the zip.

## Adding or changing a mod

1. Signatures go in a new `crates/nms-sdk/src/sigs/<mod>.rs` table, listed in `ALL` in
   `sigs/mod.rs`. Each pattern must hit exactly once; check with `sigcheck`. A function two mods
   hook must use a shared hook and be listed in `SHARED` in `crates/sigcheck/src/lib.rs`.
2. Code goes in `crates/nms-mods/src/<mod>.rs` (or a folder), implementing
   `nms_core::GameMod` (`name`, `specs`, `get`, `set`, optional `range` and `default_value`), and
   is registered in `MODS` in `crates/nms-mods/src/lib.rs`.
3. Follow every one of these rules:
   - find code by unique pattern only, never by address;
   - check the bytes before writing a patch, undo it the same way, and leave the game vanilla on
     any mismatch;
   - install hooks once; a switch is an `AtomicBool` checked in the detour; never call
     `Hook::enable` or `disable` while the game runs;
   - declare every argument the hooked function takes, stack arguments included, and forward all
     of them; too few arguments crashes the game;
   - nothing reachable from a detour may panic; use `try_lock`;
   - every behaviour change is a live switch: on by default for fixes people want, off for
     guesses and cheats;
   - log state changes once, not every frame.
   Use the helpers in `crates/nms-mods/src/common` (shared hooks, code caves, rel32, input,
   toasts) instead of new copies.
4. Setting labels and descriptions for the in-game Mods page go in
   `crates/nms-mods/src/common/menu_strings/en.json` first, then every other language file.
   `py -3 dev\menu_strings_check.py` lists missing keys. Manager strings (including the one-line
   `mod.<Name>` card description) are in `loader/locales/*.json`.
5. A mod that ships game files or a lua script gets a `mods/<Mod>/` folder with a
   `README.txt`, and its name in `RUST_STOCK_MODS` in `release.py`.

## UI mods and presets

`presets/` holds the default-size UI files the zip ships for BiggerItemGrids,
SmallerInventorySlots, CompactTooltips and CompactQuickTransfer. They are modified game files
(MBIN) built for the game build this release targets. The two size mods are rebuilt on each
player's machine by Apply sizes; the builders are in `crates/nms-launcher/src/sizes`.

A UI change that touches a game file other mods may also edit belongs in that mod's AMUMSS lua
script in `mods/<Mod>/`, so the merge at game start can combine it.

## Assets

- `py -3 dev\make_app_icon.py` writes the app icon (`rust/crates/nms-launcher/assets`).
- `py -3 dev\make_filter_icons.py` writes the Smaller Inventory Slots filter glyphs (DDS).
- `py -3 dev\make_quick_icons.py` writes the Quick Deposit and Exosuit lock glyphs (DDS).

They need no extra packages and write PNG previews to `build\previews`. The committed assets are
exactly what they produce.

## Dev-only feature

`nms-proxy` has a `selftest` cargo feature that adds a self-test section to the Mods page with
live hook and patch switches for testing the core. It enables and disables hooks at run time, which
the rules above forbid for real mods, so it is never on by default and never in a release build.

## Notes

Addresses in code comments are RVAs from the targeted game build. They move with game updates.

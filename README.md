# NMS.rs - Quality of Life Pack

NMS.rs is a mod loader and a set of quality-of-life mods for No Man's Sky, written in Rust.
A small proxy DLL loads the mods when the game starts, so there is no injector, no Python and,
on Windows, no Steam launch option. A separate manager (`nms-rs.exe`) installs, updates and parks
the mods, rebuilds the inventory sizes and merges UI scripts with other people's AMUMSS mods.

Each change to game behaviour is a live switch on the in-game Mods page (Options, then Mods).
Patches are found by a unique byte signature and checked before they are written. If the game
code moved after an update, that part stays off and the game stays vanilla.

Version 4.0.6, for game build 184271.

## Mods

| Mod | What it does |
| --- | --- |
| Bigger Item Grids | A bigger card grid on Transfer Items and Craft Product (8 by 3 in a 1.4 times larger window by default), with whole-page scrolling. |
| Smaller Inventory Slots | Smaller slots in the inventory grids so more rows show without scrolling, eight extra filter buttons by item type, optional Foods and Custom (marked items) filters, per-tab filter and scroll memory, and sort buttons on Storage Containers. |
| Compact Tooltips | Item tooltips take far less space; nothing that tells you what an item is or does is hidden. |
| Compact Quick Transfer | A much shorter Quick Transfer destination list. |
| Quick Transfer All | Shift + Quick Transfer moves every item the side filter lights up. A Deposit button quick-stacks into nearby Storage Containers. Exosuit lock pins slots, keeps a reserved count and restocks it from storage. |
| Craft from Storage | Crafting, building and charging use the Storage Containers in reach (base and freighter). Storage and freighter tabs in refiner and Nutrient Processor pickers, one Quick Transfer row per container, inventory order on the All tab. |
| Trade UI Tweaks | Q on the buy slider jumps to what you can afford, Buy and Sell keep their own list position, and Switch walks more inventories (vehicle, base storage, corvette). |
| Quick Recharge | The Recharge Equipment ring charges every listed technology in one click, with smart fuel choice and an optional never-spend list. |
| Hide Empty Hover | Hides the Modify panel that covers the grid when the cursor slides over an empty slot. |
| Sphere-Sized Mining | The Terrain Manipulator's per-dig resource cap follows the sphere size, so the big spheres mine faster. |
| Picker Tweaks | Ctrl+F searches item pickers by name, and types an exact amount for held stacks and refiners, using the game's own text box. |
| Refiner Guard | Refuses to move or delete a refiner that still holds items (vanilla deletes them). |
| Tool Mode Binds | Adds the Mining Laser, Boltcaster and Terrain Manipulator to the Quick Menu weapon list, so they can be bound to number keys. |
| Galaxy Map Filters | Extra galaxy map filters: star colour, pirate, abandoned and gas giant systems. |
| NMSrsMenu | The Mods button in the pause menu (installed by the manager). |

Each mod folder under [`mods/`](mods) has a README.txt with every switch and its default.

## Install (players)

Download the zip from the Nexus Mods page:
https://www.nexusmods.com/nomanssky/mods/4600

1. Extract the zip into your No Man's Sky game folder (the one with `Binaries` and `GAMEDATA`).
2. Run `NMS.rs\nms-rs.exe`, install the mods you want and pick your sizes.
3. Press Play in Steam.

Linux and Steam Deck (Proton, beta) need one launch option, shown in the manager:
`WINEDLLOVERRIDES="xinput9_1_0=n,b" %command%`.

If you used version 3 (NMS.py), the manager offers to migrate and backs everything up first.
Close the old NMS.py Mod Manager window before you press Migrate now. More in [docs/players.md](docs/players.md).

## Build from source (Windows)

Needs [Rust](https://rustup.rs) (stable, MSVC toolchain) and Python 3.9 or newer from python.org
(used only by the zip script and the asset scripts).

```
git clone https://github.com/skythebro/NMS.rs.git
cd NMS.rs\rust
cargo test --workspace
cargo clippy --workspace --all-targets
cd ..
py -3 release.py rust-build --version 4.0.6
py -3 release.py rust --version 4.0.6 --out %TEMP%\nmsrs-zip
```

`rust-build` builds `nms-rs.exe` and `xinput9_1_0.dll` with the `dist` profile (no debug info, no
symbols, build paths remapped) and fails if your user or repository folder name is still inside
them. It also builds the loader as `winmm.dll` and `version.dll` (Cargo features `as-winmm`,
`as-version`) for players whose `xinput9_1_0.dll` belongs to another mod, and checks that each
exports exactly what its System32 namesake exports. `rust` writes `NMS.rs-<version>.zip`, a
`SHA256SUMS.txt` inside it and a `.sha256` file next to it, plus `NMS.rs-<version>-winmm.zip` and
`NMS.rs-<version>-version.zip` ("DLL only" files). Both honour `CARGO_TARGET_DIR`.
`py -3 -I dev\test_release_rust.py` checks the zips, `py -3 -I dev\test_proxy_variants.py` calls
through the winmm and version DLLs.

For a quick local test, `cargo build --release -p nms-launcher -p nms-proxy`, then
`target\release\nms-rs.exe install` from inside a game folder copies the DLL into `Binaries`
(quit the game first; it keeps the DLL locked).

## Project layout

```
rust/                       Cargo workspace
  crates/nms-core           pattern scan, live patches, hooks, settings, logs, settings pipe
  crates/nms-sdk            signature tables, one module per mod
  crates/nms-mods           the mods (one GameMod each) and shared hooks
  crates/nms-proxy          xinput9_1_0.dll (or winmm.dll, version.dll): boot thread, registers the mods
  crates/nms-launcher       nms-rs.exe: manager window and CLI (install, mods, sizes, merge, migrate)
  crates/nms-amumss         reads AMUMSS lua scripts and applies their change tables to MXML
  crates/sigcheck           offline check of every signature against an NMS.exe
mods/<Mod>/                 shipped mod files (AMUMSS lua, textures) and the mod's README
presets/                    default-size UI game files (MBIN) the zip ships for the four UI mods
loader/locales/             manager strings, one JSON per language (compiled into nms-rs.exe)
dev/                        test_release_rust.py, menu_strings_check.py, icon generators
docs/                       player and modder documentation
release.py                  builds the shipped binaries and the release zip
```

## Signatures and fail-closed patches

No address is hard-coded. Each mod's signatures live in `nms-sdk` and are searched in a load-time
copy of the game code; a signature must match exactly once. A byte patch checks the original
bytes before writing and finds itself again by its patched bytes to undo, so every switch can be
turned on and off live. Any mismatch logs "not changed" and leaves the game vanilla. Hooks are
installed once at start; a switch is an atomic flag checked inside the detour, and a function
hooked by two mods goes through one shared hook.

After a game update, run `cargo run -q -p sigcheck --release` (from `rust\`, inside a game folder,
or pass the path to `NMS.exe`). `--compare OLD.exe NEW.exe` lists which signatures broke, per mod.
Details: [docs/how-it-works.md](docs/how-it-works.md) and [docs/modding.md](docs/modding.md).

## Credits

- NMS.rs reimplements ideas and behaviour from [NMS.py](https://github.com/monkeyman192/NMS.py)
  and [pyMHF](https://github.com/monkeyman192/pyMHF) by monkeyman192 (MIT, notice in
  [rust/THIRD-PARTY-NOTICES.txt](rust/THIRD-PARTY-NOTICES.txt)).
- [MBINCompiler](https://github.com/monkeyman192/MBINCompiler) and
  [HGPAKtool](https://github.com/monkeyman192/HGPAKtool) by monkeyman192 are not bundled. The
  manager downloads them from their GitHub releases when needed, pinned to one release and checked
  against a SHA-256 hash before anything is saved (`rust/crates/nms-launcher/src/tools.rs`).
- The manager window uses [egui](https://github.com/emilk/egui) with a CPU renderer
  (egui_software_backend). Hooks use [retour](https://github.com/Hpmason/retour-rs). Other
  dependencies are listed in `rust/Cargo.lock`.
- The UI merge reads AMUMSS-style lua mod scripts, so NMS.rs mods and other people's UI mods can
  share a game file.

No Man's Sky is a trademark of Hello Games. This project is not affiliated with Hello Games.

## Support

All mods are free. If they help you and you want to support more updates:
https://ko-fi.com/realskye

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

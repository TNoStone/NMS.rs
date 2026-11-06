# NMS.rs for players

## Install

1. Download the zip from https://www.nexusmods.com/nomanssky/mods/4600.
2. Extract it into the No Man's Sky game folder, the one that holds `Binaries` and `GAMEDATA`.
   You should now have `Binaries\xinput9_1_0.dll` and `NMS.rs\nms-rs.exe`.
3. Run `NMS.rs\nms-rs.exe`. This is the manager. Install the mods you want and pick your sizes.
4. Press Play in Steam (or Play in the manager). No launch option is needed on Windows.

Every mod arrives parked: its files wait in `NMS.rs\stock` until you click Install. On GOG and
other stores the loader starts with any copy of the game, but only Steam is tested.

### Another mod already uses `Binaries\xinput9_1_0.dll`

Only one file can have that name. Keep the other mod's file (do not let the NMS.rs zip replace
it), then also download one "DLL only" file from the Nexus page and extract it into the game
folder:

- `NMS.rs-<version>-winmm.zip` puts the loader in `Binaries\winmm.dll`. Do not use it together
  with Snap Angles' own `winmm.dll` version; only one of the two can be there.
- `NMS.rs-<version>-version.zip` puts it in `Binaries\version.dll`.

Keep one NMS.rs loader file in `Binaries`. With two, only the first one runs the mods and the
manager shows a warning. On Proton the launch option uses that name, for example
`WINEDLLOVERRIDES="winmm=n,b" %command%`; the manager shows the right one.

### Linux and Steam Deck (Proton, beta)

Wine prefers its own xinput DLL, so the game needs one launch option:

```
WINEDLLOVERRIDES="xinput9_1_0=n,b" %command%
```

Without it the game starts but the mods do not load. The manager shows this line and can set it.

## The manager (nms-rs.exe)

- Mods: one card per mod with a live, parked or update dot and an on/off switch. Install
  copies a mod from `NMS.rs\stock` into `GAMEDATA\MODS\<Mod>`; Park moves it to `NMS.rs\parked`
  so the game no longer sees it. Update (or Update all) refreshes a mod from stock and keeps any
  file you edited yourself.
- Sizes: the grid size of Bigger Item Grids and the slot size of Smaller Inventory Slots.
  See [Sizes](#sizes).
- Settings: language, text size, Pack logs (zips the logs for a bug report), Open logs
  folder, migration from version 3 and clean-up of old Python files.

The manager does not need to stay open while you play. Close the game before installing,
updating, parking or applying sizes; the manager refuses to park or apply sizes while `NMS.exe`
runs.

## The in-game Mods page

Options, then Mods, in the main menu and in the pause menu (the pause-menu button comes from the
small NMSrsMenu mod, which the manager installs). It lists every mod's switches, sliders and
choices in the game's language, with Apply, Cancel changes and Reset. Every switch applies live,
including Write log. Settings are saved in `GAMEDATA\MODS\<Mod>\<module>.json`; a new install
writes its own defaults on first start, and your settings are never overwritten by an update.

A mod that is parked (its folder is not in `GAMEDATA\MODS`) is off: its code does not hook the
game, and the page shows "Off: parked". Install it in the manager to turn it on.

The NMS.rs section has the loader's own switch, Hide mod warning (on by default), which skips
the game's "You appear to be running a modded version" screen at start. The tamper warning and the
crash dialog's Disable Mods button stay as in vanilla.

## Sizes

Bigger Item Grids and Smaller Inventory Slots change game UI files, so their size is fixed by
the files in their folder. The zip ships the defaults: an 8 by 3 grid in a 1.4 times larger
window, and 70% slots with the extra filter buttons.

To change them, close the game, open the manager, expand Sizes and press Apply sizes. The manager
then builds the files from your own installed game: it unpacks the vanilla UI files with
HGPAKtool, decompiles them with MBINCompiler, edits them and compiles them again into the two mod
folders. Your pick is saved in `NMS.rs\sizes.json`. Limits:

- grid: window scale 1.00 to 1.40, columns times rows at most 99, rows `auto` or a number;
- slots: 50% to 100% (100% is vanilla), extra filter buttons optional.

The same from a command line: `nms-rs sizes apply --columns 8 --rows auto --window 1.4 --slots 70`
(add `--no-filters` to leave the side bar alone).

The two tools are downloaded once, from their official GitHub releases, pinned to one version and
checked against a SHA-256 hash; a mismatch installs nothing. Nothing else in NMS.rs uses the network.

After a game update, press Apply sizes again if the inventory looks wrong; the update may have
changed the vanilla files the sizes were built from.

## Other people's UI mods

Many UI mods ship an AMUMSS lua script. When the game starts, NMS.rs merges every such script in
`GAMEDATA\MODS` (ours and other people's) that touch the same game file, and writes the result to
`GAMEDATA\MODS\z_Combined Mods`. It only runs again when something changed.

- `NMS.rs\conflicts.txt` lists finished MBIN files that two mods both replace without a script.
  Those cannot be merged; pick one.
- `NMS.rs\merge_order.txt`: one mod folder name per line; those are applied last, so their
  changes win.

## Coming from version 3 (NMS.py)

The manager finds an old NMS.py setup and asks first. Close the old NMS.py Mod Manager window
before you press Migrate now; the manager waits while it is open. Migrate now then:

1. backs up your settings and edited files to `NMS.rs\backup\migrate-<time>`;
2. carries the language, merge order, sizes and parked mods over to `NMS.rs`;
3. updates your mods (files you edited stay);
4. removes the old Python mod files, the old Steam launch option and the `NMS.py` folder.

Your mod settings, edited lua files and other people's mods are kept. Running it again finds
nothing and says so. Migration is also in Settings, and on the command line as `nms-rs migrate`.
While a mod's old `.py` file is still in its folder, NMS.rs leaves that mod off (status "Python
copy present"), so the two loaders never hook the same function.

## After a game update

Each mod checks the game code it uses at every start. If something moved, that mod (or that one
part of it) stays off, its status line on the Mods page and its log say why, and the rest of the
game works as in vanilla. Nothing crashes because of a moved signature. Wait for an NMS.rs update.

## Logs and problems

- `NMS.rs\logs\core.log`: what the loader did at start.
- `GAMEDATA\MODS\<Mod>\<module>.log`: per mod, when its Write log switch is on.
- Pack logs in the manager's Settings tab zips them for a bug report.
- Kill switch: `nms-rs disable` (or an empty file named `disabled` in `NMS.rs\`) keeps the game
  vanilla without uninstalling. `nms-rs enable` removes it.

## Uninstall

Delete `Binaries\xinput9_1_0.dll` (or the `winmm.dll` / `version.dll` NMS.rs file) and the
`NMS.rs` folder, then delete the mod folders you installed from `GAMEDATA\MODS` (and
`z_Combined Mods`). `nms-rs uninstall` removes our DLL under any of those names, and never
another mod's file.

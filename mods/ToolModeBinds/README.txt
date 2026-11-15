ToolModeBinds
=============

The Quick Menu's "Change Secondary Weapon" list only shows secondary weapons
(Plasma Launcher, Geology Cannon, Paralysis Mortar and so on). The Mining
Laser, the Boltcaster and the Terrain Manipulator are main (primary) modes,
so vanilla only reaches them with the weapon-switch key.

ToolModeBinds adds rows for those three tools to that list. Picking one
switches your main multi-tool mode, the same as the weapon-switch key would.
Your secondary weapon stays as it is: the secondary fire button still fires
the one you had. Picking a secondary row works exactly as in vanilla.

The list title still reads "Change Secondary Weapon". A tool row shows only
when your multi-tool has that tool installed.

The Quick Menu's Recharge list is not changed by this mod. Vanilla lists the
multi-tool tech of the mode you hold and of your secondary weapon there, so
after you pick the Mining Laser or Terrain Manipulator it shows that tool.

Goes in No Man's Sky\GAMEDATA\MODS\ToolModeBinds. Needs NMS.rs 4.0.

Tip: binding to number keys
---------------------------

Vanilla already lets you bind multi-tool modes: open the Quick Menu, go into
Change Secondary Weapon, hover a row and press Ctrl+digit (for example
Ctrl+7). Close the menu; pressing 7 then switches to that mode. The tool rows
this mod adds bind the same way, and a bound tool goes through the same menu
action, so pressing its key switches your main mode.

Switches (in-game Mods page, ToolModeBinds)
-------------------------------------------

- Tools in the weapon menu (on). Turns the mod on or off, live. Off puts the
  game code back exactly as it was. A number key you bound to a tool row
  stays bound while the mod is off: it plays the menu click and does
  nothing, as vanilla does for any bound row it cannot equip. Rebind the
  key (Ctrl+digit on another row) or turn the mod back on. The same holds
  after you remove or uninstall the mod: the bind is saved by the game, and
  pressing that key then only plays the click.
- Mining Laser, Boltcaster, Terrain Manipulator (on). Which rows to add.
- More modes (empty). Open Select modes and toggle the extra weapon modes
  you want to add. Use Back and then Apply on Mods to save. The picker works
  from the title screen. In game, Edit modes as text also accepts mode names
  or numbers, including modes outside the picker. These extra modes are
  experimental.
- Write log (off): writes each row let into the list and each mode switch
  to tool_mode_binds.log.

Earlier test builds: if you ran a test build of NMS.rs 4.0 before this
release, ToolModeBinds may still be switched off from then (that build
shipped it off, and your saved setting is kept). Open the Mods page and check
that "Tools in the weapon menu" is on.

After a game update the mod checks that the game code it uses is the same. If
not, it stays off (the log says why) and the game works as in vanilla.

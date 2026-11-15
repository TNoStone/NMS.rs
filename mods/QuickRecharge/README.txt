QuickRecharge
=============

Recharge every technology the Quick Menu Recharge Equipment ring lists.

Goes in No Man's Sky\GAMEDATA\MODS\QuickRecharge. Needs the NMS.rs loader.

Switches (in-game Mods page, QuickRecharge)
-------------------------------------------

- Charge menu: all (on): click the Recharge Equipment bolt to charge every
  listed tech. It spends your fuel the same way the game's own Charge row
  does. For each tech it takes the strongest fuel that still fits in the
  gap, as many as fit, then the next one down, and tops off with the
  cheapest. A nearly empty tech gets the rich fuel. A nearly full one
  gets the cheap fuel. Applies live.
- Substances first (off): spends substances (Ammonia, Uranium) before
  craftables (Ion Battery) when both can charge the tech. Inside each
  class it still uses strongest that fits, then cheapest to top off. Off:
  strongest fuel first, mixed. If no substance can charge that tech, it
  still uses the product. Applies live.
- Never spend (all off by default): choose Select fuels and turn on the
  fuels you want Charge menu: all to leave alone. The picker also works
  from the title screen. Hover over Select fuels to review your exclusions.
  Go Back to Mods and choose Apply to save. Undo Changes or Back > Discard
  drops the draft. Mods > Reset to Default stages the shipped settings.
- In gameplay, Edit exclusions as text edits the same comma-separated list.
  It accepts names and IDs. Either accepted ID or the display name excludes
  the same fuel for Sodium, Sodium Nitrate, Ion Battery and Pyrite. The text
  box can reopen values of up to 63 UTF-8 bytes. For longer lists, use the
  selector or the NMS.rs set command. The Clear buttons stage an empty list.
  The original text row remains available if only the text box is supported.
- Free fill (no cost) (off): fills every listed tech for nothing, like the
  free-charge game setting. Only used when this is on.
- Play charge sound (on): the game's own charge sound, once per recharge-all.
- Show notice (on): a notification in the middle of the screen that
  lists what was used, for example
  "Recharged 1 tech: 8 Pure Ferrite, 4 Ferrite Dust".
- Write log (off): turns quick_recharge.log on or off. The log lists
  every fuel each tech could use and what was taken.

After a game update the mod checks that the game code it uses is the same. If not,
it stays off (the log says why) and the game works as in vanilla.

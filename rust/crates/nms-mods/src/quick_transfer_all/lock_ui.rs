//! Lock button, dim cave, pin/unpin, persist, Confirm swallow, hold timing.

use nms_core::mem::{self, read_f32, read_i32, read_u8, read_u64, MIN_ADDR};
use nms_core::process::ProcessImage;

use crate::common::custom_filter;
use crate::common::hooks::do_inventory_slots;

use super::caves::{install_deposit_hover, install_lock_dim};
use super::inv::{
    child_named, lock_cargo_store, ordered_slots, COLUMNS, CONFIRM_INSTANT_ACTIONS, CONFIRM_NEAR_ZERO,
    HOVER_STALE_SECS, INV_SUIT_KINDS, PAGE_BUSY, SCREEN_FROM_FILTER,
};
use super::notify::{install_button_notify, lock_click_ok};
use super::runtime::Runtime;

impl Runtime {
    pub fn install_features(&mut self, image: &ProcessImage) {
        match install_deposit_hover(image, self.deposit, self.same_item, false, self.lang) {
            Ok(hover) => {
                self.deposit_hover = Some(hover);
                self.log
                    .info("Deposit button: the side bar's own hover test, with a tooltip");
            }
            Err(error) => self.log.info(&format!("Deposit button is off: {error}")),
        }
        match install_button_notify(image, self.lang) {
            Ok(note) => {
                if note.has_visual() {
                    note.set_visual(false);
                    self.log.info("button confirm visual: ring when the game needs a hold");
                } else {
                    self.log.info("button confirm visual not changed");
                }
                self.log.info("Deposit and lock: notice without Confirm");
                self.button_notify = Some(note);
            }
            Err(error) => self.log.info(&format!("button notice not changed: {error}")),
        }
        if lock_click_ok(image) {
            self.lock_click_ok = true;
            self.log.info("suit lock click: pin without pickup");
        } else {
            self.lock_click_ok = false;
            self.log.info("suit lock click not changed");
        }
        match install_lock_dim(image) {
            Ok(dim) => {
                self.lock_dim = Some(dim);
                self.log.info("suit lock dim: fade while lock mode is on");
            }
            Err(error) => self.log.info(&format!("suit lock dim not changed: {error}")),
        }
        self.log.info("suit lock icon parked");
        self.log
            .info("suit lock surplus: Quick Transfer and Deposit leave the reserved count");
        self.log
            .info("suit lock restock: on Exosuit open, nearby storage only");
        self.log.info("suit lock sort: pinned slots stay put");
        if self.suit_lock {
            self.log
                .info("Exosuit lock mode: setting on, lock mode off until the side-bar button is clicked");
        }
    }

    pub fn page_screen(&self, page: u64) -> Option<i32> {
        let game = self.game.as_ref()?;
        if page < MIN_ADDR as u64 {
            return None;
        }
        read_i32((page as usize).wrapping_add(game.filter_field as usize).wrapping_add(SCREEN_FROM_FILTER))
    }

    pub fn refresh_lock_button(&mut self) {
        let show = self.suit_lock && self.lock_screen == Some(0);
        if let Some(hover) = &self.deposit_hover {
            hover.set_lock_enabled(show);
        }
        if Some(show) == self.lock_button_shown {
            return;
        }
        self.lock_button_shown = Some(show);
        self.log
            .info(&format!("suit lock button: {}", if show { "Exosuit" } else { "hidden" }));
    }

    pub fn save_id(&self) -> u64 {
        let player = self.game.as_ref().map(|g| g.player_state()).unwrap_or(0);
        if player < MIN_ADDR {
            return 0;
        }
        read_u64(player.wrapping_add(super::inv::SAVE_ID_AT)).unwrap_or(0)
    }

    pub fn lock_mode_off(&mut self, reason: &str) {
        if !self.lock_mode {
            return;
        }
        self.lock_mode = false;
        self.lock_hits.clear();
        if let Some(dim) = self.lock_dim.as_mut() {
            dim.set_enabled(false);
            dim.undo();
        }
        if let Some(note) = &self.button_notify {
            note.set_lock_mode(false);
        }
        self.log.info(&format!("suit lock: mode off ({reason})"));
    }

    pub fn lock_mode_on(&mut self) {
        if self.lock_mode {
            return;
        }
        self.lock_mode = true;
        if let Some(note) = &self.button_notify {
            note.set_lock_mode(true);
        }
        self.sync_dim(self.lock_store as usize);
        self.log.info("suit lock: mode on");
    }

    pub fn sync_dim(&mut self, store: usize) {
        if !self.lock_mode || self.lock_screen != Some(0) {
            if let Some(dim) = self.lock_dim.as_mut() {
                dim.set_enabled(false);
                dim.undo();
            }
            return;
        }
        let inv = if lock_cargo_store(store) {
            self.store_inv(store)
        } else {
            None
        };
        let Some(dim) = self.lock_dim.as_mut() else {
            return;
        };
        if !dim.apply() {
            if !self.lock_dim_note {
                self.lock_dim_note = true;
                self.log.info("suit lock dim not changed: the dim test is busy");
            }
            return;
        }
        dim.load_pins(&self.lock_pins, inv);
        dim.set_enabled(true);
    }

    pub fn reload_pins(&mut self, save_id: u64) {
        self.follow_reset(true);
        self.lock_save_id = Some(save_id);
        self.lock_pins.clear();
        if !self.lock_persist_ok {
            if !self.lock_persist_note {
                self.lock_persist_note = true;
                self.log.info("suit lock persist not changed");
            }
            return;
        }
        if save_id == 0 {
            return;
        }
        let path = self.persist_dir.join(format!("suit_lock_{save_id:016x}.json"));
        let data = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.log
                    .info(&format!("suit lock: persist loaded 0 pins for {save_id:016x}"));
                return;
            }
            Err(_) => {
                self.log.info("suit lock: persist file was not readable");
                return;
            }
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
            return;
        };
        let Some(pins) = value.get("pins").and_then(|v| v.as_array()) else {
            return;
        };
        let mut loaded = 0i32;
        for raw in pins {
            let Some(obj) = raw.as_object() else {
                continue;
            };
            let (Some(inv), Some(x), Some(y), Some(id), Some(kind), Some(count)) = (
                obj.get("inv").and_then(|v| v.as_i64()),
                obj.get("x").and_then(|v| v.as_i64()),
                obj.get("y").and_then(|v| v.as_i64()),
                obj.get("id").and_then(|v| v.as_str()),
                obj.get("type").and_then(|v| v.as_i64()),
                obj.get("count").and_then(|v| v.as_i64()),
            ) else {
                continue;
            };
            let Ok(bytes) = hex_decode(id) else {
                continue;
            };
            if bytes.len() != 16 || !(0..32).contains(&x) || !(0..32).contains(&y) {
                continue;
            }
            let mut ident = [0u8; 16];
            ident.copy_from_slice(&bytes);
            self.lock_pins.insert(
                (inv as i32, x as i32, y as i32),
                super::inv::Pin {
                    id: ident,
                    kind: kind as i32,
                    count: count as i32,
                },
            );
            loaded += 1;
        }
        self.log
            .info(&format!("suit lock: persist loaded {loaded} pins for {save_id:016x}"));
    }

    pub fn write_pins(&self) {
        let Some(save_id) = self.lock_save_id.filter(|&id| id != 0) else {
            return;
        };
        if !self.lock_persist_ok {
            return;
        }
        let path = self.persist_dir.join(format!("suit_lock_{save_id:016x}.json"));
        let mut pins = Vec::new();
        let mut keys: Vec<_> = self.lock_pins.keys().copied().collect();
        keys.sort_unstable();
        for (inv, x, y) in keys {
            if inv < 0 {
                continue;
            }
            if let Some(pin) = self.lock_pins.get(&(inv, x, y)) {
                pins.push(serde_json::json!({
                    "inv": inv,
                    "x": x,
                    "y": y,
                    "id": hex_encode(&pin.id),
                    "type": pin.kind,
                    "count": pin.count,
                }));
            }
        }
        let body = serde_json::json!({"save_id": format!("{save_id:016x}"), "pins": pins});
        if let Ok(text) = serde_json::to_string_pretty(&body) {
            let _ = std::fs::write(path, format!("{text}\n"));
            self.log
                .info(&format!("suit lock: persist saved {} pins for {save_id:016x}", pins.len()));
        }
    }

    pub fn pin_click(&mut self, store: usize, x: i32, y: i32) {
        if !lock_cargo_store(store) {
            return;
        }
        let inv = self.store_inv(store);
        let key = (inv.unwrap_or(-(store as i32)), x, y);
        if let Some(pin) = self.lock_pins.remove(&key) {
            self.log
                .info(&format!("suit lock: unpin {} at {x},{y}", pin.name()));
            if inv.is_some() {
                self.write_pins();
            }
            self.sync_dim(store);
            return;
        }
        let element = super::inv::slot_at(store, x, y);
        let Some(ident) = super::inv::item_identity(element) else {
            return;
        };
        let count = nms_core::mem::read_i32(element.wrapping_add(super::inv::ELEMENT_AMOUNT)).unwrap_or(0);
        self.lock_pins.insert(
            key,
            super::inv::Pin {
                id: ident.1,
                kind: ident.0,
                count,
            },
        );
        self.log.info(&format!(
            "suit lock: pin {} at {x},{y} count {count}",
            super::inv::id_name(&ident.1)
        ));
        if inv.is_some() {
            self.write_pins();
        }
        self.sync_dim(store);
    }

    pub fn resolve_confirm_time(&mut self) {
        if self.confirm_resolved {
            return;
        }
        self.confirm_resolved = true;
        self.confirm_ok = self.confirm_time_addr >= MIN_ADDR;
        if !self.confirm_ok {
            self.log.info("button confirm time not changed");
            return;
        }
        let (time_s, _mul, slow) = self.confirm_durations();
        let extra = if self.instant_ui() { " instant UI" } else { "" };
        if extra.is_empty() && time_s > CONFIRM_INSTANT_ACTIONS {
            self.log.info(&format!("button confirm time: hold {slow:.2}s{extra}"));
        } else {
            self.log.info(&format!("button confirm time: tap{extra}"));
        }
    }

    fn confirm_durations(&self) -> (f32, f32, f32) {
        let mut pad = read_f32(self.confirm_time_addr).unwrap_or(0.0);
        let mut mul = read_f32(self.confirm_mouse_addr).unwrap_or(0.0);
        let mut slow = read_f32(self.confirm_slow_addr).unwrap_or(0.0);
        if !pad.is_finite() || pad < 0.0 {
            pad = 0.0;
        }
        if !mul.is_finite() || mul < 0.0 {
            mul = 0.0;
        }
        if !slow.is_finite() || slow < 0.0 {
            slow = 0.0;
        }
        (pad, mul, slow)
    }

    fn instant_ui(&self) -> bool {
        if self.instant_ui_addr < MIN_ADDR {
            return false;
        }
        read_u8(self.instant_ui_addr).unwrap_or(0) != 0
    }

    pub fn confirm_need(&mut self, _mouse: bool) -> f32 {
        self.resolve_confirm_time();
        if !self.confirm_ok {
            return 0.0;
        }
        if self.instant_ui() {
            return 0.0;
        }
        let (pad, _mul, slow) = self.confirm_durations();
        if pad <= CONFIRM_INSTANT_ACTIONS {
            return 0.0;
        }
        let need = if slow > CONFIRM_NEAR_ZERO { slow } else { pad };
        if need < 0.0 {
            0.0
        } else {
            need
        }
    }

    pub fn confirm_ready(&mut self, started: Option<std::time::Instant>, now: std::time::Instant, mouse: bool) -> bool {
        let need = self.confirm_need(mouse);
        if need <= CONFIRM_NEAR_ZERO {
            return true;
        }
        started.is_some_and(|at| now.duration_since(at).as_secs_f32() >= need)
    }

    pub fn fire_lock_button(&mut self) {
        if self.lock_mode {
            self.lock_mode_off("button");
        } else {
            self.lock_mode_on();
        }
        if let Some(note) = &self.button_notify {
            note.set_lock_fire(true);
        }
    }

    fn swallow_confirm(&mut self, page: u64) {
        if page < MIN_ADDR as u64 {
            return;
        }
        let busy = (page as usize).wrapping_add(PAGE_BUSY);
        let value = read_u8(busy).unwrap_or(0);
        self.confirm_restore = Some((busy, value));
        let _ = unsafe { mem::write(busy, 0u8) };
    }

    fn restore_confirm(&mut self) {
        if let Some((busy, value)) = self.confirm_restore.take() {
            let _ = unsafe { mem::write(busy, value) };
        }
    }

    fn hovered_slot(&self, layer: usize, columns: usize) -> Option<(i32, i32)> {
        if layer < MIN_ADDR || columns == 0 {
            return None;
        }
        for (index, widget) in ordered_slots(layer).into_iter().enumerate() {
            let y = (index / columns) as i32;
            let x = (index % columns) as i32;
            if self.game.as_ref().is_some_and(|g| g.cursor_hit(widget as u64)) {
                return Some((x, y));
            }
            let item = child_named(widget, "ITEM");
            if item >= MIN_ADDR && self.game.as_ref().is_some_and(|g| g.cursor_hit(item as u64)) {
                return Some((x, y));
            }
        }
        None
    }

    fn sync_button_visual(&mut self) {
        let Some(notify) = self.button_notify.as_ref() else {
            return;
        };
        if !notify.has_visual() {
            return;
        }
        let on = self.confirm_need(false) > CONFIRM_NEAR_ZERO;
        if Some(on) == self.button_visual_on {
            return;
        }
        self.button_visual_on = Some(on);
        if let Some(notify) = &self.button_notify {
            notify.set_visual(on);
        }
        self.log.info(&format!(
            "button confirm visual: {}",
            if on { "hold ring" } else { "tap, no ring" }
        ));
    }

    pub fn before_slots(&mut self, args: &mut do_inventory_slots::Args) {
        let page = args.page;
        let store = args.inventory as usize;
        let layer = args.layer;
        let now = std::time::Instant::now();
        self.sync_button_visual();
        let stale = self
            .lock_seen_at
            .is_none_or(|at| now.duration_since(at).as_secs_f64() > HOVER_STALE_SECS);
        if self.lock_mode && stale {
            self.lock_mode_off("inventory closed");
            self.known_stores.clear();
            self.restock_done = false;
            if self.follow_hold.is_none() {
                self.follow_reset(false);
            }
        } else if stale {
            self.known_stores.clear();
            self.restock_done = false;
            if self.follow_hold.is_none() {
                self.follow_reset(false);
            }
        }
        self.lock_seen_at = Some(now);
        let screen = self.page_screen(page);
        if screen != Some(0) {
            self.lock_mode_off("left Exosuit");
            self.restock_done = false;
            if self.follow_live() && self.follow_suit_store >= MIN_ADDR {
                self.follow_scan(self.follow_suit_store);
            }
            if self.follow_hold.is_none() {
                self.follow_reset(false);
            }
        }
        self.lock_screen = screen;
        self.refresh_lock_button();
        if screen == Some(0) && store >= MIN_ADDR && layer >= MIN_ADDR as u64 {
            self.lock_page = page;
            self.lock_store = store as u64;
            self.lock_layer = layer;
            if self.store_inv(store).is_some_and(|k| INV_SUIT_KINDS.contains(&k)) {
                self.follow_suit_store = store;
            }
            if self.suit_lock {
                let save_id = self.save_id();
                if self.lock_save_id != Some(save_id) {
                    self.reload_pins(save_id);
                }
            }
            if self.lock_mode {
                self.sync_dim(store);
            }
            if self.suit_lock && !self.restock_done && lock_cargo_store(store) {
                self.follow_resolve(store);
                self.restock_done = true;
                let pulled = self.restock_suit(page, store);
                if pulled > 0 {
                    self.restock_osd = pulled;
                }
            }
        } else if let Some(dim) = self.lock_dim.as_mut() {
            dim.set_enabled(false);
            dim.undo();
        }
        if self.deposit_hover.is_some() && self.suit_lock && !self.lock_click_off {
            self.resolve_confirm_time();
            let click = self.click_held();
            let hovered = self.deposit_hover.as_mut().is_some_and(|h| h.lock_hovered()) && screen == Some(0);
            if !self.confirm_ok {
                if click && !self.lock_down && hovered {
                    self.fire_lock_button();
                }
                self.lock_down = click;
            } else if !click {
                self.lock_wait_at = None;
                self.lock_down = false;
            } else {
                if hovered && self.lock_wait_at.is_none() {
                    self.lock_wait_at = Some(now);
                }
                if hovered && !self.lock_down && self.confirm_ready(self.lock_wait_at, now, self.mouse_down()) {
                    self.fire_lock_button();
                    self.lock_down = true;
                }
            }
        }
        if self.lock_click_ok && !self.lock_click_off && self.lock_mode && screen == Some(0) && page >= MIN_ADDR as u64 && lock_cargo_store(store)
        {
            let skip_swallow = self.button_visual()
                && self
                    .deposit_hover
                    .as_mut()
                    .is_some_and(|h| h.lock_hovered() || h.hovered());
            if !skip_swallow {
                self.swallow_confirm(page);
            }
            let slot_down = self.slot_select_held();
            if slot_down && !self.slot_down {
                self.slot_tries = 4;
            } else if !slot_down {
                self.slot_tries = 0;
            }
            self.slot_down = slot_down;
        }
        self.custom_before(page, store, layer as usize, screen);
        if self.game.is_none() || self.deposit_hover.is_none() || !self.deposit || self.deposit_click_off {
            return;
        }
        self.resolve_confirm_time();
        let held = self.deposit_held();
        let press = self.click_held();
        if held && !self.deposit_hover_logged {
            self.deposit_hover_logged = true;
            self.log.info("Deposit button: cursor is on it");
        }
        if store >= MIN_ADDR {
            self.known_stores.insert(store);
        }
        if !self.confirm_ok {
            if held && !self.deposit_down {
                self.deposit_stores.clear();
                self.deposit_sent = 0;
                self.deposit_lit = 0;
                self.deposit_tried = 0;
                self.deposit_block.clear();
                self.try_deposit_page(page, store);
            }
            self.deposit_down = press;
            if held && store >= MIN_ADDR && self.deposit_stores.is_empty() {
                self.try_deposit_page(page, store);
            }
        } else if !press {
            self.deposit_wait_at = None;
            self.deposit_down = false;
        } else {
            if held && self.deposit_wait_at.is_none() {
                self.deposit_wait_at = Some(now);
            }
            let ready = self.confirm_ready(self.deposit_wait_at, now, self.mouse_down());
            if held && !self.deposit_down && ready {
                self.deposit_stores.clear();
                self.deposit_sent = 0;
                self.deposit_lit = 0;
                self.deposit_tried = 0;
                self.deposit_block.clear();
                self.try_deposit_page(page, store);
                self.deposit_down = true;
            }
            if held && store >= MIN_ADDR && self.deposit_stores.is_empty() && self.deposit_down {
                self.try_deposit_page(page, store);
            }
        }
    }

    pub fn after_slots(&mut self, args: &do_inventory_slots::Args) {
        let page = args.page;
        let store = args.inventory as usize;
        self.restore_confirm();
        self.consume_button_clicks(page, store);
        let pulled = std::mem::replace(&mut self.restock_osd, 0);
        if pulled > 0 {
            if let Some(note) = &self.button_notify {
                note.show_text(&super::text::count(self.lang, "restocked", pulled));
            }
        }
        self.follow_watch_away_store(store);
        self.follow_scan(store);
        self.custom_after(args);
        if self.slot_tries <= 0 || self.lock_click_off || !self.lock_mode {
            return;
        }
        let layer = args.layer as usize;
        let screen = self.page_screen(page);
        if screen != Some(0) || store < MIN_ADDR || layer < MIN_ADDR {
            self.slot_tries -= 1;
            return;
        }
        if !lock_cargo_store(store) {
            return;
        }
        if let Some(slot) = self.hovered_slot(layer, COLUMNS) {
            self.slot_tries = 0;
            self.pin_click(store, slot.0, slot.1);
        } else {
            self.slot_tries -= 1;
        }
    }

    /// Mark mode of the Custom filter: on any inventory grid while the page filter is Custom.
    /// Exosuit lock mode wins on the Exosuit page.
    fn custom_active(&self, page: u64, screen: Option<i32>) -> bool {
        if !custom_filter::custom_on() || !self.lock_click_ok || self.lock_click_off {
            return false;
        }
        if self.lock_mode && screen == Some(0) {
            return false;
        }
        let Some(game) = self.game.as_ref() else {
            return false;
        };
        if page < MIN_ADDR as u64 {
            return false;
        }
        read_i32((page as usize).wrapping_add(game.filter_field as usize)) == Some(custom_filter::CUSTOM_ID)
    }

    /// Same click language as lock mode: the game's slot Confirm is swallowed for this grid (the item
    /// is not picked up) and a click edge arms `mark_tries` for the cursor test after the grid draws.
    /// While the player carries a stack (picked up on another tab or grid) the click is a drop: the
    /// Confirm is left to the game, as with the vanilla filters, and no mark is armed.
    fn custom_before(&mut self, page: u64, store: usize, layer: usize, screen: Option<i32>) {
        self.custom_drop_stale(store);
        if !self.custom_active(page, screen) || store < MIN_ADDR || layer < MIN_ADDR {
            return;
        }
        if super::inv::page_holding_item(page as usize) {
            if !self.mark_drop_logged {
                self.mark_drop_logged = true;
                self.log.info("Custom filter: a stack is held, the click is a drop (no mark)");
            }
            self.mark_down = self.slot_select_held();
            self.mark_tries = 0;
            return;
        }
        self.mark_drop_logged = false;
        let skip_swallow = self.button_visual()
            && self
                .deposit_hover
                .as_mut()
                .is_some_and(|h| h.lock_hovered() || h.hovered());
        if !skip_swallow && self.confirm_restore.is_none() {
            self.swallow_confirm(page);
        }
        let down = self.slot_select_held();
        if down && !self.mark_down {
            self.mark_tries = 4;
        } else if !down {
            self.mark_tries = 0;
        }
        self.mark_down = down;
    }

    fn custom_after(&mut self, args: &do_inventory_slots::Args) {
        if self.mark_tries <= 0 {
            return;
        }
        let page = args.page;
        let store = args.inventory as usize;
        let layer = args.layer as usize;
        let screen = self.page_screen(page);
        if !self.custom_active(page, screen) || store < MIN_ADDR || layer < MIN_ADDR {
            self.mark_tries -= 1;
            return;
        }
        let columns = if (1..=32).contains(&args.slots_wide) {
            args.slots_wide as usize
        } else {
            COLUMNS
        };
        if let Some((x, y)) = self.hovered_slot(layer, columns) {
            self.mark_tries = 0;
            self.mark_click(store, x, y, columns);
        } else {
            self.mark_tries -= 1;
        }
    }

    /// Per-slot marks of this store whose slot no longer holds the marked item (moved away, used up,
    /// swapped, sorted) are dropped. Runs on every draw of the grid, whatever its filter.
    fn custom_drop_stale(&mut self, store: usize) {
        if !custom_filter::custom_on() || !custom_filter::slot_mode() || store < MIN_ADDR {
            return;
        }
        let marks = custom_filter::slot_marks_in(store);
        if marks.is_empty() {
            return;
        }
        let Some(now) = super::inv::store_contents(store) else {
            return;
        };
        let stale = custom_filter::stale_slot_marks(&marks, |x, y| now.get(&(x, y)).copied());
        if stale.is_empty() {
            return;
        }
        let gone = custom_filter::unmark_slots(store, &stale);
        if gone > 0 {
            self.log.info(&format!(
                "Custom filter: {gone} marked slot(s) changed item, unmarked, {} marked",
                custom_filter::len()
            ));
        }
    }

    fn mark_click(&mut self, store: usize, x: i32, y: i32, columns: usize) {
        let element = super::inv::slot_at(store, x, y);
        let Some((kind, id)) = super::inv::item_identity(element) else {
            return;
        };
        let name = super::inv::id_name(&id);
        let per = if custom_filter::slot_mode() { "stack" } else { "item" };
        match custom_filter::toggle(kind, id, custom_filter::Slot { store, x, y }) {
            custom_filter::Toggle::Marked => self.log.info(&format!(
                "Custom filter: marked {name} ({per}) at {x},{y} (row of {columns}), {} marked",
                custom_filter::len()
            )),
            custom_filter::Toggle::Unmarked => self.log.info(&format!(
                "Custom filter: unmarked {name} at {x},{y}, {} marked",
                custom_filter::len()
            )),
            custom_filter::Toggle::Full => self.log.info(&format!(
                "Custom filter: {} items are marked already, unmark one first",
                custom_filter::MAX_MARKS
            )),
            custom_filter::Toggle::Busy => {}
        }
    }

    fn button_visual(&mut self) -> bool {
        self.button_notify.as_ref().is_some_and(|n| n.has_visual()) && self.confirm_need(false) > CONFIRM_NEAR_ZERO
    }

    fn slot_select_held(&mut self) -> bool {
        if self
            .deposit_hover
            .as_mut()
            .is_some_and(|h| h.lock_hovered() || h.hovered())
        {
            return false;
        }
        self.click_held()
    }

    fn consume_button_clicks(&mut self, page: u64, store: usize) {
        if !self.button_visual() {
            return;
        }
        let take_lock = self.button_notify.as_ref().is_some_and(|n| n.take_lock_click());
        let take_dep = self.button_notify.as_ref().is_some_and(|n| n.take_dep_click());
        if take_lock && !self.lock_down && self.suit_lock && !self.lock_click_off && self.page_screen(page) == Some(0) {
            self.fire_lock_button();
            self.lock_down = true;
        }
        if take_dep && !self.deposit_down && self.deposit && !self.deposit_click_off {
            self.deposit_stores.clear();
            self.deposit_sent = 0;
            self.deposit_lit = 0;
            self.deposit_tried = 0;
            self.deposit_block.clear();
            self.try_deposit_page(page, store);
            self.deposit_down = true;
        }
    }

    pub fn apply_live_toggles(&mut self) {
        if let Some(hover) = &self.deposit_hover {
            hover.set_enabled(self.deposit);
            hover.set_same(self.same_item);
        }
        if !self.suit_lock {
            self.lock_mode_off("setting off");
        }
        self.refresh_lock_button();
        if !self.suit_lock_follow {
            self.follow_reset(true);
        }
    }
}

fn hex_decode(text: &str) -> Result<Vec<u8>, ()> {
    if !text.len().is_multiple_of(2) {
        return Err(());
    }
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::super::inv::Pin;
    use super::Runtime;
    use nms_core::log::Log;
    use std::sync::Arc;

    fn runtime(dir: &std::path::Path) -> Runtime {
        let mut rt = Runtime::new(Arc::new(Log::disabled()), dir.to_path_buf());
        rt.lock_persist_ok = true;
        rt
    }

    /// Save ids with the top bit set (about half of all saves) once looked negative, so pins were
    /// never written and every start reloaded an old file.
    #[test]
    fn pins_survive_a_restart_for_a_top_bit_save_id() {
        let dir = std::env::temp_dir().join(format!("nmsrs-pins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let id = 0xe06c_bd2e_0551_92b1u64;

        let mut rt = runtime(&dir);
        rt.reload_pins(id);
        rt.lock_pins.insert((0, 2, 1), Pin { id: *b"ULTRAPRODX40\0\0\0\0", kind: 2, count: 7 });
        rt.write_pins();
        assert!(dir.join(format!("suit_lock_{id:016x}.json")).is_file());

        let mut again = runtime(&dir);
        again.reload_pins(id);
        let pin = again.lock_pins.get(&(0, 2, 1)).expect("pin reloaded");
        assert_eq!((pin.kind, pin.count), (2, 7));

        // No save: nothing is written.
        let mut none = runtime(&dir);
        none.reload_pins(0);
        none.lock_pins.insert((0, 0, 0), Pin { id: [1; 16], kind: 2, count: 1 });
        none.write_pins();
        assert!(!dir.join(format!("suit_lock_{:016x}.json", 0)).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

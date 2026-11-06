//! Sell floor, Award skip/scale, voice NOP, pickup Add redirect, lock-follow.

use nms_core::mem::{self, read_i32, MIN_ADDR};

use super::caves::write_code;
use super::inv::{
    empty_slot, item_identity, items, pair_kind, slot_at, store_push, store_remove_at, ELEMENT_AMOUNT, ELEMENT_INDEX,
    ELEMENT_SIZE, INV_COUNT, INV_SUIT_ALL, INV_SUIT_KINDS, STORE_COUNT,
    STORE_DATA, STORE_STRIDE,
};
use super::lock_inv::inv_is_suit;
use super::runtime::Runtime;

const NOP5: [u8; 5] = [0x90, 0x90, 0x90, 0x90, 0x90];
type Occupied = std::collections::HashMap<(i32, i32), Option<(i32, [u8; 16])>>;

impl Runtime {
    pub fn follow_reset(&mut self, baselines: bool) {
        self.follow_hold = None;
        self.follow_seen.clear();
        self.follow_inv = None;
        if baselines {
            self.follow_baseline.clear();
        }
    }

    pub fn follow_live(&self) -> bool {
        self.follow_ok && self.suit_lock && self.suit_lock_follow
    }

    fn follow_occupied(&self, store: usize) -> Occupied {
        let mut now = Occupied::new();
        for (y, x, element) in items(store) {
            if let Some(ident) = item_identity(element) {
                now.insert((x, y), Some(ident));
            }
        }
        if let Some(inv) = self.store_inv(store) {
            for &(pinv, x, y) in self.lock_pins.keys() {
                if pinv == inv {
                    now.entry((x, y)).or_insert(None);
                }
            }
        }
        now
    }

    fn follow_dests(
        now: &Occupied,
        prev: &Occupied,
        ident: (i32, [u8; 16]),
        src: (i32, i32),
    ) -> Vec<(i32, i32)> {
        now.iter()
            .filter_map(|(&(dx, dy), got)| {
                if *got == Some(ident) && (dx, dy) != src && prev.get(&(dx, dy)).copied().flatten() != Some(ident) {
                    Some((dx, dy))
                } else {
                    None
                }
            })
            .collect()
    }

    fn follow_apply_moves(&mut self, store: usize, moves: &[(i32, i32, i32, i32)]) {
        let Some(inv) = self.store_inv(store) else {
            return;
        };
        if !INV_SUIT_KINDS.contains(&inv) || moves.is_empty() {
            return;
        }
        let mut planned = Vec::new();
        for &(src_x, src_y, dest_x, dest_y) in moves {
            let src = (inv, src_x, src_y);
            let dest = (inv, dest_x, dest_y);
            let Some(pin) = self.lock_pins.get(&src).copied() else {
                continue;
            };
            if dest == src {
                continue;
            }
            let occupant = slot_at(store, dest_x, dest_y);
            let ident = if occupant >= MIN_ADDR {
                item_identity(occupant)
            } else {
                None
            };
            if ident != Some(pin.ident()) {
                continue;
            }
            planned.push((src, dest, pin));
        }
        if planned.is_empty() {
            return;
        }
        for (src, _, _) in &planned {
            self.lock_pins.remove(src);
        }
        for (src, dest, pin) in planned {
            self.lock_pins.insert(dest, pin);
            self.log.info(&format!(
                "suit lock: follow {} {},{} to {},{}",
                pin.name(),
                src.1,
                src.2,
                dest.1,
                dest.2
            ));
        }
        self.write_pins();
        self.sync_dim(store);
    }

    fn follow_clear(&mut self, key: (i32, i32, i32), reason: &str) {
        if let Some(pin) = self.lock_pins.remove(&key) {
            self.log.info(&format!(
                "suit lock: follow clear {} at {},{} ({reason})",
                pin.name(),
                key.1,
                key.2
            ));
            self.write_pins();
            if self.lock_store >= MIN_ADDR as u64 {
                self.sync_dim(self.lock_store as usize);
            }
        }
    }

    fn follow_ident_amount(&self, store: usize, ident: (i32, [u8; 16])) -> i32 {
        let mut total = 0i32;
        for (_y, _x, element) in items(store) {
            if item_identity(element) == Some(ident) {
                total += read_i32(element.wrapping_add(ELEMENT_AMOUNT)).unwrap_or(0);
            }
        }
        total
    }

    fn follow_ident_away(&self, ident: (i32, [u8; 16])) -> std::collections::HashMap<i32, i32> {
        let player = self.game.as_ref().map(|g| g.player_state()).unwrap_or(0);
        let mut found = std::collections::HashMap::new();
        if player < MIN_ADDR {
            return found;
        }
        for kind in 0..INV_COUNT {
            if INV_SUIT_ALL.contains(&kind) {
                continue;
            }
            let store = self.kind_store(player, kind);
            if store >= MIN_ADDR {
                found.insert(kind, self.follow_ident_amount(store, ident));
            }
        }
        for (key, base) in [(100i32, 0x7458usize), (200, 0x8FC8)] {
            for index in 0..12 {
                let store = player.wrapping_add(base).wrapping_add(index * STORE_STRIDE);
                found.insert(key + index as i32, self.follow_ident_amount(store, ident));
            }
        }
        found
    }

    fn follow_away_grew(&self, key: Option<(i32, i32, i32)>) -> bool {
        let Some(key) = key.or(self.follow_hold) else {
            return false;
        };
        let Some(baseline) = self.follow_baseline.get(&key) else {
            return false;
        };
        let Some(pin) = self.lock_pins.get(&key) else {
            return false;
        };
        let now = self.follow_ident_away(pin.ident());
        let kinds: std::collections::HashSet<i32> = now.keys().chain(baseline.keys()).copied().collect();
        kinds.into_iter().any(|k| now.get(&k).copied().unwrap_or(0) > baseline.get(&k).copied().unwrap_or(0))
    }

    pub fn follow_watch_away_store(&mut self, store: usize) {
        if !self.follow_live() || self.follow_hold.is_none() || store < MIN_ADDR {
            return;
        }
        if self.store_inv(store).is_some_and(inv_is_suit) {
            return;
        }
        if self.follow_away_grew(self.follow_hold) {
            if let Some(key) = self.follow_hold.take() {
                self.follow_clear(key, "left suit");
            }
        }
    }

    pub fn follow_resolve(&mut self, store: usize) {
        if !self.follow_live() || store < MIN_ADDR {
            return;
        }
        let Some(inv) = self.store_inv(store) else {
            return;
        };
        if !INV_SUIT_KINDS.contains(&inv) {
            return;
        }
        let now = self.follow_occupied(store);
        let mut moves = Vec::new();
        let pins: Vec<_> = self.lock_pins.iter().map(|(k, v)| (*k, *v)).collect();
        for ((pinv, x, y), pin) in pins {
            if pinv != inv {
                continue;
            }
            let ident = pin.ident();
            if now.get(&(x, y)).copied().flatten() == Some(ident) {
                self.follow_baseline.insert((inv, x, y), self.follow_ident_away(ident));
                if self.follow_hold == Some((inv, x, y)) {
                    self.follow_hold = None;
                }
                continue;
            }
            let dests = if self.follow_seen.is_empty() {
                Vec::new()
            } else {
                Self::follow_dests(&now, &self.follow_seen, ident, (x, y))
            };
            if dests.len() == 1 {
                moves.push((x, y, dests[0].0, dests[0].1));
            } else if self.follow_away_grew(Some((inv, x, y))) {
                self.follow_clear((inv, x, y), "left suit");
                if self.follow_hold == Some((inv, x, y)) {
                    self.follow_hold = None;
                }
            }
        }
        if !moves.is_empty() {
            self.follow_apply_moves(store, &moves);
            self.follow_hold = None;
        }
    }

    pub fn follow_scan(&mut self, store: usize) {
        if !self.follow_live() {
            self.follow_reset(false);
            return;
        }
        if store < MIN_ADDR {
            return;
        }
        let Some(inv) = self.store_inv(store) else {
            return;
        };
        if !INV_SUIT_KINDS.contains(&inv) {
            return;
        }
        let now = self.follow_occupied(store);
        if Some(inv) != self.follow_inv {
            self.follow_seen.clear();
            self.follow_inv = Some(inv);
        }
        let prev = self.follow_seen.clone();
        let busy = self.restock_busy || self.qt_busy || self.deposit_busy || !self.sort_hold.is_empty();
        let mut moves = Vec::new();
        let pins: Vec<_> = self.lock_pins.iter().map(|(k, v)| (*k, *v)).collect();
        for ((pinv, x, y), pin) in pins {
            if pinv != inv {
                continue;
            }
            let ident = pin.ident();
            let src_now = now.get(&(x, y)).copied().flatten();
            let src_was = prev.get(&(x, y)).copied().flatten();
            let held = self.follow_hold == Some((inv, x, y));
            if src_now == Some(ident) {
                self.follow_baseline.insert((inv, x, y), self.follow_ident_away(ident));
                if held {
                    self.follow_hold = None;
                }
                continue;
            }
            if busy || prev.is_empty() {
                continue;
            }
            if src_was != Some(ident) && !held {
                continue;
            }
            if src_was == Some(ident) {
                self.follow_hold = Some((inv, x, y));
            }
            let dests = Self::follow_dests(&now, &prev, ident, (x, y));
            if dests.len() == 1 {
                moves.push((x, y, dests[0].0, dests[0].1));
            } else if self.follow_away_grew(Some((inv, x, y))) {
                self.follow_clear((inv, x, y), "left suit");
                if self.follow_hold == Some((inv, x, y)) {
                    self.follow_hold = None;
                }
            }
        }
        if !moves.is_empty() {
            self.follow_apply_moves(store, &moves);
            self.follow_hold = None;
        }
        self.follow_seen = now;
    }

    pub fn ensure_lock_pins(&mut self) -> bool {
        if !self.suit_lock {
            return false;
        }
        let save_id = self.save_id();
        if self.lock_save_id != Some(save_id) {
            self.reload_pins(save_id);
        }
        !self.lock_pins.is_empty()
    }

    pub fn before_add(&mut self, store: u64, result: u64, element: u64) -> Option<(u64, u64, u64)> {
        if !self.pickup_ok || !self.suit_lock {
            return None;
        }
        let store_i = store as usize;
        let elem = element as usize;
        if store_i < MIN_ADDR || elem < MIN_ADDR {
            return None;
        }
        if !self.ensure_lock_pins() {
            return None;
        }
        let inv = self.store_inv(store_i)?;
        if !INV_SUIT_KINDS.contains(&inv) {
            return None;
        }
        let ident = item_identity(elem)?;
        let dest_x = read_i32(elem.wrapping_add(ELEMENT_INDEX)).unwrap_or(-1);
        let dest_y = read_i32(elem.wrapping_add(ELEMENT_INDEX).wrapping_add(4)).unwrap_or(-1);
        if dest_x >= 0 && dest_y >= 0 {
            return None;
        }
        let first = empty_slot(store_i, &std::collections::HashSet::new())?;
        let pin = self.lock_pins.get(&(inv, first.0, first.1));
        if pin.is_none() || pin.is_some_and(|p| p.ident() == ident) {
            return None;
        }
        let skip: std::collections::HashSet<(i32, i32)> = self
            .lock_pins
            .iter()
            .filter(|((pinv, _, _), saved)| *pinv == inv && saved.ident() != ident)
            .map(|((_, x, y), _)| (*x, *y))
            .collect();
        let hole = empty_slot(store_i, &skip);
        if let Some(raw) = nms_core::mem::read_bytes(elem, ELEMENT_SIZE) {
            self.pickup_hold.copy_from_slice(&raw);
        }
        if let Some(hole) = hole {
            self.pickup_hold[ELEMENT_INDEX..ELEMENT_INDEX + 4].copy_from_slice(&hole.0.to_le_bytes());
            self.pickup_hold[ELEMENT_INDEX + 4..ELEMENT_INDEX + 8].copy_from_slice(&hole.1.to_le_bytes());
            self.log.info(&format!("suit lock: pickup skip {},{}", first.0, first.1));
        } else {
            self.pickup_hold[ELEMENT_INDEX..ELEMENT_INDEX + 4].copy_from_slice(&31i32.to_le_bytes());
            self.pickup_hold[ELEMENT_INDEX + 4..ELEMENT_INDEX + 8].copy_from_slice(&31i32.to_le_bytes());
            self.log
                .info(&format!("suit lock: pickup bounced {},{}", first.0, first.1));
        }
        Some((store, result, self.pickup_hold.as_mut_ptr() as u64))
    }

    fn sell_pin_match(&self, pin: &super::inv::Pin, ident: (i32, [u8; 16])) -> bool {
        pin.id == ident.1 && (ident.0 < 0 || pin.kind == ident.0)
    }

    fn sell_matching(&self, store: usize, ident: (i32, [u8; 16])) -> Vec<(i32, i32, usize)> {
        let mut found = Vec::new();
        for (y, x, element) in items(store) {
            let Some(got) = item_identity(element) else {
                continue;
            };
            if ident.0 < 0 {
                if got.1 == ident.1 {
                    found.push((x, y, element));
                }
            } else if got == ident {
                found.push((x, y, element));
            }
        }
        found
    }

    fn sell_visible(&self, store: usize, inv: Option<i32>, ident: (i32, [u8; 16])) -> i32 {
        let mut total = 0i32;
        for (x, y, element) in self.sell_matching(store, ident) {
            let mut have = read_i32(element.wrapping_add(ELEMENT_AMOUNT)).unwrap_or(0);
            if let Some(inv) = inv {
                if let Some(pin) = self.lock_pins.get(&(inv, x, y)) {
                    if self.sell_pin_match(pin, ident) {
                        have -= have.min(pin.count.max(0));
                    }
                }
            }
            if have > 0 {
                total += have;
            }
        }
        total
    }

    fn sell_hide(&mut self, store: usize, inv: Option<i32>, ident: (i32, [u8; 16])) -> i32 {
        let Some(inv) = inv else {
            return 0;
        };
        if !INV_SUIT_KINDS.contains(&inv) {
            return 0;
        }
        let Some(count) = nms_core::mem::read_u32(store.wrapping_add(STORE_COUNT)) else {
            return 0;
        };
        let Some(data) = nms_core::mem::read_u64(store.wrapping_add(STORE_DATA)) else {
            return 0;
        };
        if data < MIN_ADDR as u64 || count == 0 || count > 10000 {
            return 0;
        }
        let mut hidden = 0i32;
        let mut n = count as i32 - 1;
        while n >= 0 {
            let element = (data as usize).wrapping_add((n as usize).wrapping_mul(ELEMENT_SIZE));
            if item_identity(element) != Some(ident) {
                n -= 1;
                continue;
            }
            let x = read_i32(element.wrapping_add(ELEMENT_INDEX)).unwrap_or(0);
            let y = read_i32(element.wrapping_add(ELEMENT_INDEX).wrapping_add(4)).unwrap_or(0);
            let pin = self.lock_pins.get(&(inv, x, y)).copied();
            let Some(pin) = pin else {
                n -= 1;
                continue;
            };
            if !self.sell_pin_match(&pin, ident) {
                n -= 1;
                continue;
            }
            let have = read_i32(element.wrapping_add(ELEMENT_AMOUNT)).unwrap_or(0);
            let keep = have.min(pin.count.max(0));
            if keep <= 0 {
                n -= 1;
                continue;
            }
            let Some(mut hold) = nms_core::mem::read_bytes(element, ELEMENT_SIZE) else {
                n -= 1;
                continue;
            };
            hold[ELEMENT_AMOUNT..ELEMENT_AMOUNT + 4].copy_from_slice(&keep.to_le_bytes());
            let extra = have - keep;
            if extra > 0 {
                let _ = unsafe { mem::write(element.wrapping_add(ELEMENT_AMOUNT), extra) };
            } else {
                store_remove_at(store, n);
            }
            self.sell_hide_hold.push((store, x, y, hold));
            hidden += keep;
            n -= 1;
        }
        hidden
    }

    fn sell_restore(&mut self) {
        let hold = std::mem::take(&mut self.sell_hide_hold);
        for (store, x, y, blob) in hold {
            let keep = i32::from_le_bytes([blob[ELEMENT_AMOUNT], blob[ELEMENT_AMOUNT + 1], blob[ELEMENT_AMOUNT + 2], blob[ELEMENT_AMOUNT + 3]]);
            let occupant = slot_at(store, x, y);
            if occupant >= MIN_ADDR {
                if let Some(id) = nms_core::mem::read_bytes(occupant, 16) {
                    if id == blob[..16] {
                        let have = read_i32(occupant.wrapping_add(ELEMENT_AMOUNT)).unwrap_or(0);
                        let _ = unsafe { mem::write(occupant.wrapping_add(ELEMENT_AMOUNT), have.wrapping_add(keep)) };
                        self.log.info(&format!("suit lock: sell restore {keep} at {x},{y}"));
                        continue;
                    }
                }
                self.log
                    .info(&format!("suit lock: sell restore skipped {x},{y}, slot occupied"));
                continue;
            }
            if store_push(store, &blob) {
                self.log.info(&format!("suit lock: sell restore {keep} at {x},{y}"));
            } else {
                self.log.info(&format!("suit lock: sell restore failed {x},{y}"));
            }
        }
    }

    fn sell_voice(&self, on: bool) {
        for site in &self.sell_voice_sites {
            let raw = if on { site.old } else { NOP5 };
            let got = nms_core::mem::read_bytes(site.site, 5).unwrap_or_default();
            if got == raw {
                continue;
            }
            let _ = write_code(site.site, &raw);
        }
    }

    pub fn before_take(&mut self, player: u64, kind: i32, element: u64, index: i64) {
        if !self.sell_ok || !self.suit_lock || index != -1 {
            return;
        }
        let player_i = player as usize;
        let elem = element as usize;
        if player_i < MIN_ADDR || elem < MIN_ADDR {
            return;
        }
        let first = pair_kind(kind).is_some();
        if first {
            self.sell_restore();
            self.sell_pay = None;
            self.sell_voice(true);
        }
        if !self.ensure_lock_pins() {
            return;
        }
        let Some(ident) = item_identity(elem) else {
            return;
        };
        let store = self.kind_store(player_i, kind);
        if store < MIN_ADDR {
            return;
        }
        let inv = if (0..INV_COUNT).contains(&kind) { Some(kind) } else { None };
        let want = read_i32(elem.wrapping_add(ELEMENT_AMOUNT)).unwrap_or(0);
        if want <= 0 {
            return;
        }
        if first {
            let mut visible = self.sell_visible(store, inv, ident);
            if let Some(pair) = pair_kind(kind) {
                let other = self.kind_store(player_i, pair);
                if other >= MIN_ADDR {
                    visible += self.sell_visible(other, Some(pair), ident);
                }
            }
            if visible < want {
                let _ = unsafe { mem::write(elem.wrapping_add(ELEMENT_AMOUNT), visible) };
                self.sell_pay = Some((want, visible));
                if visible != 0 {
                    self.log.info(&format!("suit lock: sell floor {visible} of {want}"));
                } else {
                    self.log.info("suit lock: sell blocked");
                }
            }
        }
        let hid = self.sell_hide(store, inv, ident);
        if hid != 0 {
            self.log.info(&format!("suit lock: sell hold {hid} on kind {kind}"));
        }
    }

    pub fn after_take(&mut self, _player: u64, _kind: i32, _element: u64, _index: i64, _ret: i32) {
        self.sell_restore();
    }

    fn sell_scaled(&self, value: i32) -> Option<i32> {
        let (old, new) = self.sell_pay?;
        if old <= 0 || new >= old {
            return None;
        }
        Some(value.saturating_mul(new) / old)
    }

    fn sell_scale(&mut self, change: i32) -> Option<i32> {
        let pay = self.sell_pay;
        let scaled = self.sell_scaled(change);
        let sold = pay.map(|p| p.1).unwrap_or(0);
        self.sell_pay = None;
        let scaled = scaled?;
        self.log.info(&format!("suit lock: sell pay {scaled} of {change}"));
        if scaled <= 0 {
            self.sell_voice(false);
            self.log.info("suit lock: sell voice off");
        }
        let lang = self.lang;
        if let Some(note) = self.button_notify.as_mut() {
            let text = if sold <= 0 {
                super::text::get(lang, "sale_blocked").to_string()
            } else {
                super::text::count(lang, "sold", sold)
            };
            note.show_now(&text);
        }
        Some(scaled)
    }

    pub fn before_sell_demand(
        &mut self,
        owner: u64,
        item: u64,
        flag: i32,
        qty: i32,
        extra: u64,
        mark: u64,
    ) -> Option<(u64, u64, i32, i32, u64, u64)> {
        let scaled = self.sell_scaled(qty)?;
        self.log.info(&format!("suit lock: sell demand {scaled} of {qty}"));
        Some((owner, item, flag, scaled, extra, mark))
    }

    pub fn before_award_units(&mut self, _player: u64, amount: i32) -> Option<(i32, bool)> {
        match self.sell_scale(amount) {
            Some(scaled) if scaled <= 0 => Some((0, true)),
            Some(scaled) => Some((scaled, false)),
            None => None,
        }
    }

    pub fn before_award_nanites(&mut self, player: u64, amount: i32) -> Option<(i32, bool)> {
        self.before_award_units(player, amount)
    }
}

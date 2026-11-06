//! Shared QTA state. Feature methods live in send_all, deposit, lock_ui, lock_inv, lock_shop.
#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use nms_core::log::Log;

use std::path::PathBuf;

use crate::common::input::SteamInput;

use super::caves::{ButtonNotify, DepositHover, LockDim};
use super::game::{Game, VoiceSite};
use super::inv::Pin;

pub const ELEMENT_SIZE: usize = 0x30;
pub const VK_SHIFT: i32 = 0x10;
pub const VK_CTRL: i32 = 0x11;
pub const VK_ALT: i32 = 0x12;
pub const MOUSE_LEFT: i32 = 0x01;

pub struct Runtime {
    pub log: Arc<Log>,
    pub game: Option<Game>,
    pub qt_vk: Option<i32>,
    pub qt_busy: bool,
    pub deposit_busy: bool,
    pub restock_busy: bool,
    pub deposit: bool,
    pub same_item: bool,
    pub suit_lock: bool,
    pub suit_lock_follow: bool,
    pub deposit_down: bool,
    pub deposit_stores: HashSet<usize>,
    pub known_stores: HashSet<usize>,
    pub deposit_sent: i32,
    pub deposit_lit: i32,
    pub deposit_tried: i32,
    pub deposit_block: String,
    pub deposit_self_logged: i32,
    pub deposit_hover_logged: bool,
    pub deposit_click_off: bool,
    pub deposit_scope_off: bool,
    pub deposit_wait_at: Option<Instant>,
    pub lock_mode: bool,
    pub lock_down: bool,
    pub lock_wait_at: Option<Instant>,
    pub confirm_time_addr: usize,
    pub confirm_mouse_addr: usize,
    pub confirm_slow_addr: usize,
    pub instant_ui_addr: usize,
    pub confirm_resolved: bool,
    pub confirm_ok: bool,
    pub button_visual_on: Option<bool>,
    pub lock_click_off: bool,
    pub lock_click_ok: bool,
    pub lock_cursor: Option<usize>,
    pub slot_down: bool,
    pub slot_tries: i32,
    pub mark_down: bool,
    pub mark_tries: i32,
    /// "click is a drop" was logged for the current held stack.
    pub mark_drop_logged: bool,
    pub lock_persist_ok: bool,
    pub lock_wallet: Option<usize>,
    /// Save id the pins were loaded for; `None` until the first load. The id is a u64: a signed
    /// copy made ids with the top bit set look negative, and pins were never written for them.
    pub lock_save_id: Option<u64>,
    pub persist_dir: PathBuf,
    /// Game language (locale code) for the toasts and tooltips; read once at start (`text`).
    pub lang: &'static str,
    pub lock_pins: HashMap<(i32, i32, i32), Pin>,
    pub lock_hits: HashMap<usize, (i32, i32)>,
    pub lock_page: u64,
    pub lock_store: u64,
    pub lock_layer: u64,
    pub lock_screen: Option<i32>,
    pub lock_seen_at: Option<Instant>,
    pub lock_dim_note: bool,
    pub lock_persist_note: bool,
    pub confirm_restore: Option<(usize, u8)>,
    pub qt_picked: Option<(i32, i32)>,
    pub lock_button_shown: Option<bool>,
    pub restock_done: bool,
    pub restock_osd: i32,
    pub restock_note: bool,
    pub sort_hold: Vec<(i32, i32, Vec<u8>)>,
    pub sort_store: usize,
    pub surplus_hold: [i32; 2],
    pub pickup_hold: [u8; ELEMENT_SIZE],
    pub pickup_ok: bool,
    pub follow_ok: bool,
    pub follow_hold: Option<(i32, i32, i32)>,
    pub follow_seen: HashMap<(i32, i32), Option<(i32, [u8; 16])>>,
    pub follow_inv: Option<i32>,
    pub follow_baseline: HashMap<(i32, i32, i32), HashMap<i32, i32>>,
    pub follow_suit_store: usize,
    pub sell_ok: bool,
    pub sell_hide_hold: Vec<(usize, i32, i32, Vec<u8>)>,
    pub sell_pay: Option<(i32, i32)>,
    pub sell_voice_sites: Vec<VoiceSite>,
    pub sell_voice_ok: bool,
    pub steam: Option<SteamInput>,
    pub steam_hold: Vec<u64>,
    pub steam_select: Vec<u64>,
    pub steam_off: bool,
    pub deposit_hover: Option<DepositHover>,
    pub button_notify: Option<ButtonNotify>,
    pub lock_dim: Option<LockDim>,
}

impl Runtime {
    pub fn new(log: Arc<Log>, persist_dir: PathBuf) -> Self {
        Self {
            log,
            persist_dir,
            lang: "en",
            game: None,
            qt_vk: Some(VK_SHIFT),
            qt_busy: false,
            deposit_busy: false,
            restock_busy: false,
            deposit: true,
            same_item: true,
            suit_lock: true,
            suit_lock_follow: false,
            deposit_down: false,
            deposit_stores: HashSet::new(),
            known_stores: HashSet::new(),
            deposit_sent: 0,
            deposit_lit: 0,
            deposit_tried: 0,
            deposit_block: String::new(),
            deposit_self_logged: 0,
            deposit_hover_logged: false,
            deposit_click_off: false,
            deposit_scope_off: false,
            deposit_wait_at: None,
            lock_mode: false,
            lock_down: false,
            lock_wait_at: None,
            confirm_time_addr: 0,
            confirm_mouse_addr: 0,
            confirm_slow_addr: 0,
            instant_ui_addr: 0,
            confirm_resolved: false,
            confirm_ok: false,
            button_visual_on: None,
            lock_click_off: false,
            lock_click_ok: false,
            lock_cursor: None,
            slot_down: false,
            slot_tries: 0,
            mark_down: false,
            mark_tries: 0,
            mark_drop_logged: false,
            lock_persist_ok: false,
            lock_wallet: None,
            lock_save_id: None,
            lock_pins: HashMap::new(),
            lock_hits: HashMap::new(),
            lock_page: 0,
            lock_store: 0,
            lock_layer: 0,
            lock_screen: None,
            lock_seen_at: None,
            lock_dim_note: false,
            lock_persist_note: false,
            confirm_restore: None,
            qt_picked: None,
            lock_button_shown: None,
            restock_done: true,
            restock_osd: 0,
            restock_note: false,
            sort_hold: Vec::new(),
            sort_store: 0,
            surplus_hold: [31, 31],
            pickup_hold: [0; ELEMENT_SIZE],
            pickup_ok: false,
            follow_ok: false,
            follow_hold: None,
            follow_seen: HashMap::new(),
            follow_inv: None,
            follow_baseline: HashMap::new(),
            follow_suit_store: 0,
            sell_ok: false,
            sell_hide_hold: Vec::new(),
            sell_pay: None,
            sell_voice_sites: Vec::new(),
            sell_voice_ok: false,
            steam: None,
            steam_hold: Vec::new(),
            steam_select: Vec::new(),
            steam_off: false,
            deposit_hover: None,
            button_notify: None,
            lock_dim: None,
        }
    }

    pub fn apply_game(&mut self, game: Game) {
        self.confirm_time_addr = game.confirm_time_addr;
        self.confirm_mouse_addr = game.confirm_mouse_addr;
        self.confirm_slow_addr = game.confirm_slow_addr;
        self.instant_ui_addr = game.instant_ui_addr;
        self.confirm_ok = game.confirm_ok;
        self.lock_cursor = game.lock_cursor;
        self.lock_click_ok = game.lock_cursor.is_some();
        self.lock_persist_ok = game.lock_persist_ok;
        self.lock_wallet = game.wallet;
        self.pickup_ok = game.pickup_ok;
        self.follow_ok = game.follow_ok;
        self.sell_ok = game.sell_ok;
        self.sell_voice_ok = game.sell_voice_ok;
        self.sell_voice_sites = game.sell_voice_sites.clone();
        self.game = Some(game);
    }

    pub fn after_add(&mut self, _store: u64, _result: u64, _element: u64, _ret: u64) {}

    pub fn after_sell_demand(
        &mut self,
        _owner: u64,
        _item: u64,
        _flag: i32,
        _qty: i32,
        _extra: u64,
        _mark: u64,
        _ret: u64,
    ) {
    }

    pub fn after_award_units(&mut self, _player: u64, _amount: i32, _ret: u64) {}

    pub fn after_award_nanites(&mut self, _player: u64, _amount: i32, _ret: u64) {}
}

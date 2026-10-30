//! Out-of-game manager window (`nms-rs` / `nms-rs manager`). File work stays in `mods` and `sizes::apply`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Instant;

use egui::Color32;
use egui_software_backend::{SoftwareBackend, SoftwareBackendAppConfiguration};
use nms_core::{paths, spawn};
use serde_json::Value;

use crate::cards::{self, Filter, FILTERS};
use crate::files;
use crate::fonts;
use crate::i18n::{self, I18n};
use crate::manager_settings::{self as mset, ManagerSettings, SCALE_DEFAULT, SCALE_MAX, SCALE_MIN, ZOOM_BASE};
use crate::migrate::{self, Legacy};
use crate::mods;
use crate::palette as pal;
use crate::proxy;
use crate::sizes::apply::{self, ApplySpec};
use crate::store::{self, Store};
use crate::style as st;

const STEAM_PLAY: &str = "steam://rungameid/275850";

/// The NMS.rs badge (dev/make_app_icon.py): the window icon and the logo in the header.
const ICON_RGBA: &[u8] = include_bytes!("../assets/nms-rs-64.rgba");
const ICON_SIZE: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModRow {
    pub name: String,
    pub state: String,
    pub outdated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeFields {
    pub columns: String,
    pub rows: String,
    pub window: String,
    pub slots: String,
    pub filters: bool,
}

impl SizeFields {
    pub fn defaults() -> Self {
        Self {
            columns: "8".into(),
            rows: "auto".into(),
            window: "1.4".into(),
            slots: "70".into(),
            filters: true,
        }
    }

    pub fn clamp(&self) -> Self {
        let (columns, rows, window, slots) = clamp_size_fields(&self.columns, &self.rows, &self.window, &self.slots);
        Self {
            columns,
            rows,
            window,
            slots,
            filters: self.filters,
        }
    }

    pub fn to_spec(&self) -> ApplySpec {
        let clamped = self.clamp();
        ApplySpec {
            columns: clamped.columns.parse().unwrap_or(8),
            rows: clamped.rows,
            window: clamped.window.parse().unwrap_or(1.4),
            slots: clamped.slots.parse().unwrap_or(70.0),
            filters: clamped.filters,
        }
    }
}

pub fn clamp_size_fields(columns: &str, rows: &str, window: &str, slots: &str) -> (String, String, String, String) {
    let window_n = window.trim().parse::<f64>().unwrap_or(1.4);
    let window_c = apply::clamp_window(window_n);
    let cols_n = columns.trim().parse::<i64>().unwrap_or(8);
    let cols_c = apply::clamp_columns(cols_n, window_c);
    let rows_c = apply::clamp_rows_text(rows, cols_c);
    let slots_n = slots.trim().parse::<f64>().unwrap_or(70.0);
    let slots_c = apply::clamp_percent(slots_n);
    (cols_c.to_string(), rows_c, fmt_num(window_c), slots_c.to_string())
}

pub fn parse_status_lines(text: &str) -> Vec<ModRow> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("python leftover:") {
            continue;
        }
        let Some((name, state)) = line.rsplit_once(' ') else {
            continue;
        };
        if state != "live" && state != "parked" && state != "missing" {
            continue;
        }
        rows.push(ModRow {
            name: name.to_string(),
            state: state.to_string(),
            outdated: false,
        });
    }
    rows
}

pub fn listed_mod_names(root: &Path) -> Vec<String> {
    let mut names = mods::stock_names(root);
    for extra in mods::OUR_MODS {
        if names.iter().any(|n| n == extra) {
            continue;
        }
        if mods::mods_dir(root).join(extra).is_dir() {
            names.push((*extra).to_string());
        }
    }
    names
}

pub fn rows_from_disk(root: &Path) -> Vec<ModRow> {
    let mut rows = parse_status_lines(&mods::status(root));
    for name in listed_mod_names(root) {
        if rows.iter().any(|row| row.name == name) {
            continue;
        }
        rows.push(ModRow {
            name,
            state: "live".into(),
            outdated: false,
        });
    }
    for row in &mut rows {
        let folder = match row.state.as_str() {
            "live" => mods::mods_dir(root).join(&row.name),
            "parked" => mods::parked_dir(root).join(&row.name),
            _ => continue,
        };
        row.outdated = mods::is_outdated(root, &row.name, &folder);
    }
    rows
}

/// Installed (live or parked) mods whose per-row Update button shows: what `mods::update_all`
/// will update. The Update all button is drawn only when this is above zero.
pub fn outdated_count(rows: &[ModRow]) -> usize {
    rows.iter().filter(|row| has_update(row)).count()
}

/// An installed (live or parked) mod with a newer stock copy: its card shows Update.
pub fn has_update(row: &ModRow) -> bool {
    row.outdated && (row.state == "live" || row.state == "parked")
}

/// Splits a report line like `+ Backup done` into its mark and the text. Plain text gives `None`.
fn split_mark(line: &str) -> (Option<char>, &str) {
    let mut chars = line.chars();
    match (chars.next(), chars.next()) {
        (Some(mark @ ('+' | '=' | '!' | 'x')), Some(' ')) => (Some(mark), &line[2..]),
        _ => (None, line),
    }
}

/// Label of the Update all button, `None` when nothing is out of date (the button is hidden).
pub fn update_all_label(template: &str, rows: &[ModRow]) -> Option<String> {
    let n = outdated_count(rows);
    (n > 0).then(|| template.replace("{n}", &n.to_string()))
}

fn fmt_num(n: f64) -> String {
    let t = format!("{n:.2}");
    t.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|n| n as i64))
        .or_else(|| value.as_f64().map(|n| n.round() as i64))
        .or_else(|| value.as_str()?.parse().ok())
}

fn json_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| value.as_i64().map(|n| n as f64)).or_else(|| value.as_str()?.parse().ok())
}

fn json_rows(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        return Some(t.to_string());
    }
    json_i64(value).map(|n| n.to_string())
}

pub fn size_fields_from_json(text: &str) -> SizeFields {
    let mut fields = SizeFields::defaults();
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return fields;
    };
    if let Some(grid) = value.get("BiggerItemGrids") {
        if let Some(n) = grid.get("columns").and_then(json_i64) {
            fields.columns = n.to_string();
        }
        if let Some(rows) = grid.get("rows").and_then(json_rows) {
            fields.rows = rows;
        }
        if let Some(n) = grid.get("window_scale").and_then(json_f64) {
            fields.window = fmt_num(n);
        }
    }
    if let Some(slots) = value.get("SmallerInventorySlots") {
        if let Some(scale) = slots.get("scale").and_then(json_f64) {
            fields.slots = apply::clamp_percent(scale * 100.0).to_string();
        }
        if let Some(on) = slots.get("filters").and_then(Value::as_bool) {
            fields.filters = on;
        }
    }
    fields
}

fn load_size_fields(root: &Path) -> SizeFields {
    let path = paths::loader_dir(root).join("sizes.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return SizeFields::defaults();
    };
    size_fields_from_json(&text)
}

struct Manager {
    root: PathBuf,
    message: String,
    message_area: Area,
    rows: Vec<ModRow>,
    sizes: SizeFields,
    i18n: I18n,
    /// The last message was an error (shown red).
    failed: bool,
    /// Text size, 1.0 = the default look (egui zoom `ZOOM_BASE`), kept in `manager.json`.
    scale: f32,
    zoom_set: bool,
    /// Slider value while dragging; becomes `scale` on release.
    pending_scale: f32,
    tab: Tab,
    /// `tasklist` is slow, so the game-running check is cached for a couple of seconds.
    running: bool,
    running_checked: Option<std::time::Instant>,
    /// File work runs on its own thread so the window keeps drawing.
    job: Option<Job>,
    /// Running under Wine or Proton: show the Linux notice.
    wine: bool,
    /// Language the egui font set was last built for (`None`: not yet).
    fonts_for: Option<String>,
    /// The font set holds a system font (so going back to a Latin language must reset it).
    fonts_extra: bool,
    /// Languages whose missing-font line is already in `manager.log` (logged once per run).
    fonts_warned: Vec<String>,
    /// Keep the Sizes section open (the screenshot test sets it so every label is drawn).
    sizes_open: bool,
    /// The live Smaller Inventory Slots side bar predates the Foods and Custom buttons.
    side_bar_old: bool,
    /// Names under which an NMS.rs loader DLL sits in Binaries (`proxy::installed`).
    loaders: Vec<&'static str>,
    /// What is left of an old NMS.py (version 3) setup; `None` until the first check is back.
    legacy: Option<Legacy>,
    /// The check runs on its own thread (it walks NMS.py and reads Steam's config).
    legacy_rx: Option<mpsc::Receiver<(Legacy, bool)>>,
    /// A Steam account still has a launch option that the clean-up removes (from the same check).
    launch_ours: bool,
    /// Full text of the last result when `message` is a short summary (footer hover tooltip).
    message_detail: String,
    /// "Not now" was pressed: no panel on the Mods tab for the rest of this run.
    legacy_hidden: bool,
    /// Search box of the Mods tab.
    search: String,
    filter: Filter,
    /// Cards opened in place (full description, Update / Remove, settings).
    /// The "?" popover is open.
    help_open: bool,
    /// The NMS.rs badge shown in the header.
    logo: Option<egui::TextureHandle>,
    theme_set: bool,
    /// Card columns of the last frame, for the hysteresis of `cards::columns_stable` (a column
    /// count, not a measured size: every height is laid out fresh each frame).
    grid_cols: Option<usize>,
    /// Rects of the blocks drawn this frame by name ("card:<mod>", "footer", "search", ...), for
    /// the layout tests. Cleared at the start of every frame.
    layout: Vec<(String, egui::Rect)>,
}

/// What a card asked for this frame; carried out after the card is drawn.
enum CardAction {
    Install,
    Park,
    Update,
}

struct Job {
    area: Area,
    label: String,
    rx: mpsc::Receiver<Result<String, String>>,
    started: Instant,
}

/// Where a result message is shown: next to the thing the user just did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Area {
    Mods,
    Sizes,
    Settings,
    Play,
    Migrate,
}

/// How the footer shows a status: the dot, the top line and the text take the level's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Work in progress.
    Info,
    Success,
    Warn,
    Error,
}

impl Level {
    fn color(self) -> Color32 {
        match self {
            Level::Info => pal::PRIMARY_LIGHT,
            Level::Success => pal::SUCCESS,
            Level::Warn => pal::WARN,
            Level::Error => pal::DANGER,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Mods,
    Settings,
}

const RUNNING_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

impl Manager {
    fn new(root: PathBuf) -> Self {
        if !files::game_is_running() {
            let _ = mods::auto_install(&root);
        }
        let rows = rows_from_disk(&root);
        let side_bar_old = mods::side_bar_outdated(&root);
        let loaders = proxy::installed(&root);
        let sizes = load_size_fields(&root);
        let prefs = mset::load(&root);
        let i18n = I18n::new(&prefs.language);
        let scale = prefs.text_size;
        let mut manager = Self {
            root,
            scale,
            zoom_set: false,
            pending_scale: scale,
            running: false,
            running_checked: None,
            job: None,
            wine: is_wine(),
            fonts_for: None,
            fonts_extra: false,
            fonts_warned: Vec::new(),
            sizes_open: false,
            side_bar_old,
            loaders,
            tab: Tab::Mods,
            failed: false,
            message: String::new(),
            message_area: Area::Mods,
            rows,
            sizes,
            i18n,
            legacy: None,
            legacy_rx: None,
            launch_ours: false,
            message_detail: String::new(),
            legacy_hidden: false,
            search: String::new(),
            filter: Filter::All,
            help_open: false,
            logo: None,
            theme_set: false,
            grid_cols: None,
            layout: Vec::new(),
        };
        manager.start_detect();
        manager
    }

    /// Look for an old NMS.py setup in the background; `poll_detect` picks the answer up.
    fn start_detect(&mut self) {
        let (tx, rx) = mpsc::channel();
        let root = self.root.clone();
        std::thread::spawn(move || {
            let legacy = migrate::detect(&root, &migrate::production());
            let ours = crate::steam::has_our_option(&crate::steam::launch_option_strings());
            let _ = tx.send((legacy, ours));
        });
        self.legacy_rx = Some(rx);
    }

    /// The clean-up button has work: Python leftovers or a launch option it would remove.
    /// Unknown (check still running) counts as work, so the panel never hides a real job.
    fn cleanup_needed(&self) -> bool {
        match &self.legacy {
            Some(legacy) => !legacy.python_files.is_empty() || legacy.old_option || self.launch_ours,
            None => self.legacy_rx.is_some(),
        }
    }

    fn poll_detect(&mut self) {
        let Some(rx) = &self.legacy_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((legacy, ours)) => {
                self.launch_ours = ours;
                self.legacy = Some(legacy);
                self.legacy_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.legacy_rx = None,
        }
    }

    /// While an NMS.py folder is there, see on the game-running cadence whether the old Mod
    /// Manager window (a program from inside NMS.py) is still open, so the notice comes and goes.
    fn recheck_py_open(&mut self) {
        if self.job.is_some() || self.legacy_rx.is_some() {
            return;
        }
        if let Some(legacy) = self.legacy.as_mut() {
            if let Some(dir) = legacy.nms_py.clone() {
                legacy.py_open = migrate::py_processes(&dir);
            }
        }
    }

    fn legacy_found(&self) -> bool {
        self.legacy.as_ref().is_some_and(Legacy::found)
    }

    fn start_migrate(&mut self) {
        let code = self.i18n.code().to_string();
        let label = self.i18n.t("mig_button");
        self.start_job(Area::Migrate, label, move |root| {
            let report = migrate::migrate(root, &migrate::production());
            let text = report.render(&I18n::new(&code));
            if report.ok {
                Ok(text)
            } else {
                Err(text)
            }
        });
    }

    /// "Will be removed" bullets, the "will be kept" text and the two buttons. Returns
    /// (migrate clicked, not now clicked).
    fn legacy_body(&self, ui: &mut egui::Ui, running: bool, with_later: bool) -> (bool, bool) {
        let (mut go, mut later) = (false, false);
        let Some(legacy) = self.legacy.as_ref().filter(|l| l.found()) else {
            return (go, later);
        };
        ui.add(egui::Label::new(egui::RichText::new(self.i18n.t("mig_remove_head")).strong().color(pal::ACCENT_RED_LIGHT)).wrap());
        for line in migrate::plan_lines(legacy, &self.i18n) {
            ui.add(egui::Label::new(format!("•  {line}")).wrap());
        }
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(self.i18n.t("mig_keep_head")).strong().color(pal::SUCCESS)).wrap());
        ui.add(egui::Label::new(self.i18n.t("mig_keep_text")).wrap());
        ui.add_space(6.0);
        if !legacy.py_open.is_empty() {
            let note = egui::RichText::new(migrate::py_open_text(legacy, &self.i18n)).strong().color(pal::WARN);
            ui.add(egui::Label::new(note).wrap());
            ui.add_space(4.0);
        }
        // Long labels (de, ru, ko) each get a line of their own and wrap inside the window.
        let button = wide_button(self.i18n.t("mig_button"), Look::Primary);
        go = ui.add_enabled(!running, button).clicked();
        if with_later {
            later = ui.add(wide_button(self.i18n.t("mig_not_now"), Look::Secondary)).clicked();
        }
        (go, later)
    }

    /// First-run panel on the Mods tab while an old NMS.py setup is there (until "Not now").
    fn legacy_panel(&mut self, ui: &mut egui::Ui, running: bool) {
        let result = self.message_area == Area::Migrate && !self.message.is_empty();
        let working = self.job.as_ref().is_some_and(|j| j.area == Area::Migrate);
        if self.legacy_hidden || !(self.legacy_found() || result || working) {
            return;
        }
        let (mut go, mut later, mut close) = (false, false, false);
        egui::Frame::new()
            .fill(pal::SURFACE_2)
            .stroke(egui::Stroke::new(st::STROKE, pal::PRIMARY))
            .corner_radius(st::RADIUS_PANEL)
            .inner_margin(egui::Margin::same(st::S4 as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // Collapsible, so the card grid is one click away on a small window.
                let title = egui::RichText::new(self.i18n.t("mig_panel_title")).size(st::TYPE_HEADING).color(pal::PRIMARY_LIGHT);
                egui::CollapsingHeader::new(title).id_salt("mig_panel").default_open(true).show(ui, |ui| {
                    if self.legacy_found() {
                        ui.add(egui::Label::new(self.i18n.t("mig_intro")).wrap());
                        ui.add_space(4.0);
                        (go, later) = self.legacy_body(ui, running, true);
                        ui.add(egui::Label::new(dim(self.i18n.t("mig_later_hint"))).wrap());
                    }
                    self.result_list(ui, Area::Migrate);
                    if !self.legacy_found() && !working {
                        ui.horizontal(|ui| {
                            let rest = egui::vec2(ui.available_width(), st::CONTROL_H);
                            ui.allocate_ui_with_layout(rest, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                close = ui.add(styled_button(self.i18n.t("mig_close"), Look::Secondary)).clicked();
                            });
                        });
                    }
                });
            });
        ui.add_space(st::S3);
        if go {
            self.start_migrate();
        }
        if later || close {
            self.legacy_hidden = true;
        }
    }

    /// Settings: the same migration, any time (also after "Not now").
    fn legacy_section(&mut self, ui: &mut egui::Ui, running: bool) {
        ui.add(egui::Label::new(section_title(self.i18n.t("mig_title"))).wrap());
        match &self.legacy {
            None => {}
            Some(l) if l.found() => {
                ui.add(egui::Label::new(dim(self.i18n.t("mig_found"))).wrap());
                let (go, _) = self.legacy_body(ui, running, false);
                if go {
                    self.start_migrate();
                }
            }
            Some(_) => {
                ui.add(egui::Label::new(dim(self.i18n.t("mig_none"))).wrap());
            }
        }
        self.message_line(ui, Area::Migrate);
    }

    fn save_prefs(&self) {
        let prefs = ManagerSettings {
            language: self.i18n.code().to_string(),
            text_size: self.scale,
        };
        mset::save(&self.root, &prefs);
    }

    /// Korean, Japanese and Chinese need a system font (see `fonts`). Built when the language
    /// changes, so switching in the window works without a restart; egui uses the new set from
    /// the next frame on. A Latin language drops the system font again (frees 10 to 20 MB).
    fn sync_fonts(&mut self, ctx: &egui::Context) {
        let code = self.i18n.code().to_string();
        if self.fonts_for.as_deref() == Some(code.as_str()) {
            return;
        }
        let found = match fonts::load(&code, &fonts::font_dirs()) {
            None => None,
            Some(Ok(found)) => Some(found),
            Some(Err(line)) => {
                if !self.fonts_warned.contains(&code) {
                    log_error(&self.root, &line);
                    self.fonts_warned.push(code.clone());
                }
                None
            }
        };
        let extra = found.is_some();
        if extra || self.fonts_extra {
            ctx.set_fonts(fonts::definitions(found));
            ctx.request_repaint();
        }
        self.fonts_extra = extra;
        self.fonts_for = Some(code);
    }

    /// (code, text) for the language picker: native names where the active fonts can draw them.
    fn picker_names(&self, ctx: &egui::Context) -> Vec<(&'static str, &'static str)> {
        let font = egui::FontId::proportional(14.0);
        i18n::codes()
            .into_iter()
            .map(|(code, english, native)| {
                let name = i18n::picker_name(english, native, |text| ctx.fonts_mut(|f| f.has_glyphs(&font, text)));
                (code, name)
            })
            .collect()
    }

    fn language_row(&mut self, ui: &mut egui::Ui) {
        let current = self.i18n.code().to_string();
        let names = self.picker_names(ui.ctx());
        let shown = names.iter().find(|(c, _)| *c == current).map(|(_, n)| *n).unwrap_or("English");
        let pick = ui
            .horizontal(|ui| {
                ui.label(self.i18n.t("rs_language"));
                language_picker(ui, &names, shown, &current)
            })
            .inner;
        if pick != current {
            self.i18n.set(&pick);
            self.save_prefs();
        }
    }

    fn text_size_row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // The slider edits `pending`; the zoom changes only when the drag ends. Zooming while
        // dragging resizes the slider under the cursor and makes the value jump.
        let mut apply = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(self.i18n.t("rs_text_size"));
            let slider = egui::Slider::new(&mut self.pending_scale, SCALE_MIN..=SCALE_MAX)
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0));
            let response = ui.add(slider);
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                apply = Some(self.pending_scale);
            }
            // Same height as the slider's value box next to it.
            let reset = styled_button(self.i18n.t("rs_reset"), Look::Secondary).min_size(egui::vec2(0.0, st::CONTROL_SM_H));
            if ui.add(reset).clicked() {
                apply = Some(SCALE_DEFAULT);
            }
        });
        ui.add(egui::Label::new(dim(self.i18n.t("rs_text_size_hint"))).wrap());
        if let Some(value) = apply {
            self.scale = mset::clamp_scale(value);
            self.pending_scale = self.scale;
            ctx.set_zoom_factor(self.scale * ZOOM_BASE);
            self.save_prefs();
        }
    }

    /// Settings: one panel per topic (language and text size, logs, clean-up, old NMS.py).
    fn settings_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, running: bool) {
        panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(section_title(self.i18n.t("rs_settings")));
            ui.add_space(st::S2);
            self.language_row(ui);
            ui.add_space(st::S2);
            self.text_size_row(ui, ctx);
        });
        ui.add_space(st::S3);
        panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(section_title(self.i18n.t("rs_logs_title"))).wrap());
            ui.add(egui::Label::new(dim(self.i18n.t("rs_logs_help"))).wrap());
            ui.add_space(st::S2);
            ui.horizontal_wrapped(|ui| {
                let logs = wide_button(self.i18n.t("rs_pack_logs"), Look::Primary);
                if ui.add_enabled(!running, logs).clicked() {
                    let label = self.i18n.t("rs_pack_logs");
                    self.start_job(Area::Settings, label, |root| {
                        files::pack_logs(root).map(|path| {
                            // Shows the new zip in Explorer so it can be sent right away.
                            let _ = spawn::reveal(&path);
                            format!("{}: {}", "Logs", path.display())
                        })
                    });
                }
                if ui.add(wide_button(self.i18n.t("rs_open_logs"), Look::Secondary)).clicked() {
                    let logs_dir = paths::loader_dir(&self.root).join("logs");
                    let _ = std::fs::create_dir_all(&logs_dir);
                    let _ = spawn::gui("explorer.exe").arg(&logs_dir).spawn();
                }
            });
        });
        ui.add_space(st::S3);
        let needed = self.cleanup_needed();
        if needed {
            panel_frame().show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(egui::Label::new(section_title(self.i18n.t("rs_cleanup_title"))).wrap());
                self.cleanup_body(ui, running);
            });
        } else {
            // Nothing to clean: one closed line. It can still be opened to run the clean-up.
            panel_frame().inner_margin(egui::Margin::symmetric(st::S4 as i8, st::S2 as i8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let mut title = egui::text::LayoutJob::default();
                let font = st::font(st::TYPE_BODY);
                title.append(&self.i18n.t("rs_cleanup_title"), 0.0, egui::TextFormat { font_id: font.clone(), color: pal::PRIMARY_LIGHT, ..Default::default() });
                title.append(&self.i18n.t("rs_cleanup_nothing_hint"), st::S3, egui::TextFormat { font_id: font, color: pal::TEXT_MUTED, ..Default::default() });
                egui::CollapsingHeader::new(title).id_salt("cleanup_idle").default_open(false).show(ui, |ui| {
                    self.cleanup_body(ui, running);
                });
            });
        }
        ui.add_space(st::S3);
        panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.legacy_section(ui, running);
        });
    }

    /// Help text and the clean-up button (open panel, or the opened idle header).
    fn cleanup_body(&mut self, ui: &mut egui::Ui, running: bool) {
        ui.add(egui::Label::new(dim(self.i18n.t("rs_cleanup_help"))).wrap());
        ui.add_space(st::S2);
        let button = wide_button(self.i18n.t("rs_cleanup"), Look::Danger);
        if ui.add_enabled(!running, button).clicked() {
            let label = self.i18n.t("rs_cleanup_title");
            let words = CleanupWords::new(&self.i18n);
            self.start_job(Area::Settings, label, move |root| {
                let running = files::game_is_running();
                let python = mods::cleanup_python(root, running)?;
                let steam = mods::strip_launch(running)?;
                let detail = [python.as_str(), steam.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join("\n");
                log_error(root, &format!("clean-up:\n{detail}"));
                Ok(format!("{}{DETAIL_SEP}{detail}", cleanup_summary(&python, &steam, &words)))
            });
        }
    }

    /// First frame: apply the saved scale. Later: pick up Ctrl +/- and Ctrl 0 from egui.
    fn sync_zoom(&mut self, ctx: &egui::Context) {
        if !self.zoom_set {
            ctx.set_zoom_factor(self.scale * ZOOM_BASE);
            self.zoom_set = true;
            return;
        }
        let z = ctx.zoom_factor();
        if (z - self.scale * ZOOM_BASE).abs() > 0.005 {
            self.scale = mset::clamp_scale(z / ZOOM_BASE);
            self.pending_scale = self.scale;
            if (self.scale * ZOOM_BASE - z).abs() > 0.005 {
                ctx.set_zoom_factor(self.scale * ZOOM_BASE);
            }
            self.save_prefs();
        }
    }

    fn start_job<F>(&mut self, area: Area, label: String, work: F)
    where
        F: FnOnce(&Path) -> Result<String, String> + Send + 'static,
    {
        if self.job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let root = self.root.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work(&root));
        });
        self.job = Some(Job {
            area,
            label,
            rx,
            started: Instant::now(),
        });
    }

    fn poll_job(&mut self) {
        let Some(job) = &self.job else {
            return;
        };
        let area = job.area;
        let result = match job.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err(self.i18n.t("rs_job_stopped")),
        };
        self.job = None;
        self.message_area = area;
        self.set_result(result);
        // Any file job can change what is left of NMS.py (clean-up, migrate).
        self.start_detect();
    }

    /// The old-setup (migration) result, shown where it was started: its report is a long list.
    /// Every other result goes to the footer (`footer`).
    fn message_line(&self, ui: &mut egui::Ui, area: Area) {
        if self.message.is_empty() || self.message_area != area || self.job.is_some() {
            return;
        }
        let color = if self.failed { pal::DANGER } else { pal::SUCCESS };
        ui.add(egui::Label::new(egui::RichText::new(&self.message).color(color).strong()).wrap());
    }

    fn level(&self) -> Level {
        if self.failed {
            Level::Error
        } else {
            Level::Success
        }
    }

    /// The footer status bar on the bottom edge of the window: what is going on (working, game
    /// running) or the last result, with a dot (or a spinner) in the colour of its level and a
    /// dismiss cross for results. One fixed-height row: marker, text and cross are all centred on
    /// the row's midline, the marker starts on the gutter. Long text is cut with an ellipsis and
    /// shown whole on hover.
    fn footer(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let result = !self.message.is_empty() && self.message_area != Area::Migrate;
        // (level, text, spinner) in order of importance: a running job, an error, the game
        // running (buttons are off then), the last result.
        let text = self.message.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("  ");
        let status = if let Some(job) = &self.job {
            Some((Level::Info, format!("{}: {} ({}s)", self.i18n.t("rs_working"), job.label, job.started.elapsed().as_secs()), true))
        } else if result && self.failed {
            Some((Level::Error, text, false))
        } else if self.running {
            Some((Level::Warn, self.i18n.t("close_game"), false))
        } else if result {
            Some((self.level(), text, false))
        } else {
            None
        };
        // The dismiss cross belongs to a result, not to the job or game-running notices.
        let dismissable = result && self.job.is_none() && matches!(&status, Some((level, _, _)) if matches!(level, Level::Error | Level::Success));
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 0, pal::SURFACE_1);
        match &status {
            // A status has a 2 pt line in its colour on the top edge; idle has the plain divider.
            Some((level, _, _)) => {
                let line = egui::Rect::from_min_max(rect.left_top(), egui::pos2(rect.right(), rect.top() + 2.0));
                painter.rect_filled(line, 0, level.color());
            }
            None => st::hline(&painter, rect.left(), rect.right(), rect.top() + 0.5),
        }
        let mid = rect.center().y + 1.0;
        let marker = egui::Rect::from_center_size(egui::pos2(rect.left() + st::GUTTER + FOOTER_MARKER / 2.0, mid), egui::vec2(FOOTER_MARKER, FOOTER_MARKER));
        let text_x = marker.right() + st::S2;
        let close = egui::Rect::from_center_size(egui::pos2(rect.right() - st::GUTTER - st::CONTROL_SM_H / 2.0, mid), egui::vec2(st::CONTROL_SM_H, st::CONTROL_SM_H));
        let text_right = if dismissable { close.left() - st::S2 } else { rect.right() - st::GUTTER };
        let width = (text_right - text_x).max(40.0);
        let font = st::font(st::TYPE_BODY);
        let mut job = egui::text::LayoutJob::default();
        let fmt = |color| egui::TextFormat { font_id: font.clone(), color, ..Default::default() };
        match &status {
            Some((level, text, spinner)) => {
                if *spinner {
                    st::place(ui, "footer_spinner", marker, true, egui::Spinner::new().size(FOOTER_MARKER).color(level.color()));
                } else {
                    cards::paint_dot(&painter, marker.center(), 4.0, level.color());
                }
                let color = if *level == Level::Info { pal::TEXT } else { level.color() };
                job.append(text, 0.0, fmt(color));
                if *spinner {
                    job.append(&self.i18n.t("rs_working_hint"), st::S3, fmt(pal::TEXT_MUTED));
                }
            }
            None => {
                cards::paint_dot(&painter, marker.center(), 3.5, pal::PARKED);
                job.append(&format!("NMS.rs v{}", nms_core::ipc::VERSION), 0.0, fmt(pal::TEXT_MUTED));
            }
        }
        let full: String = job.text.clone();
        job.wrap.max_width = width;
        job.wrap.max_rows = 1;
        let galley = painter.layout_job(job);
        let elided = galley.elided;
        let text_rect = st::text_mid(&painter, galley, text_x, mid, pal::TEXT);
        // A summarised result shows its full detail on hover; other text only when cut off.
        let detail = result && self.job.is_none() && !self.message_detail.is_empty();
        if detail {
            let tip = format!("{}\n\n{}", full, self.message_detail);
            ui.interact(text_rect, egui::Id::new("footer_text"), egui::Sense::hover()).on_hover_text(tip);
        } else if elided {
            ui.interact(text_rect, egui::Id::new("footer_text"), egui::Sense::hover()).on_hover_text(full);
        }
        self.layout.push(("footer:marker".into(), marker));
        self.layout.push(("footer:text".into(), text_rect));
        if dismissable {
            self.layout.push(("footer:close".into(), close));
            if cards::close_button(ui, egui::Id::new("footer_close"), close).clicked() {
                self.message.clear();
                self.message_detail.clear();
                self.failed = false;
            }
        }
    }

    /// The last result as a tidy list: one row per line with a coloured marker. Lines that start
    /// with a report mark (`+ = ! x`, see `Report::render`) get that mark's colour; plain text
    /// uses the green or red of the whole result. The text stays selectable.
    fn result_list(&self, ui: &mut egui::Ui, area: Area) {
        if self.message.is_empty() || self.message_area != area || self.job.is_some() {
            return;
        }
        egui::Frame::new()
            .fill(pal::SURFACE_SUNKEN)
            .corner_radius(st::RADIUS_CONTROL)
            .inner_margin(egui::Margin::same(st::S2 as i8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 3.0;
                for line in self.message.lines().filter(|l| !l.trim().is_empty()) {
                    let (mark, text) = split_mark(line);
                    let color = match mark {
                        Some('+') => pal::SUCCESS,
                        Some('=') => pal::INFO,
                        Some('!') => pal::WARN,
                        Some(_) => pal::DANGER,
                        None if self.failed => pal::DANGER,
                        None => pal::SUCCESS,
                    };
                    ui.horizontal_top(|ui| {
                        let (dot, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
                        cards::paint_dot(ui.painter(), dot.center(), 3.5, color);
                        ui.add(egui::Label::new(egui::RichText::new(text).color(color)).wrap());
                    });
                }
            });
    }

    /// Linux / Proton beta notice with the one Steam launch option those users need, for the
    /// loader file they have (xinput9_1_0, or winmm / version from a "DLL only" download).
    /// Windows needs nothing.
    fn linux_note(&mut self, ui: &mut egui::Ui) {
        if !self.wine {
            return;
        }
        let name = match self.loaders.as_slice() {
            [one] => *one,
            _ => proxy::MAIN,
        };
        let option = proxy::wine_launch_option(name);
        panel_frame().fill(pal::NOTICE_FILL).stroke(egui::Stroke::new(st::STROKE, pal::ATTENTION.gamma_multiply(0.6))).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(egui::RichText::new(self.i18n.t("rs_linux_title")).strong().color(pal::WARN)).wrap());
            // Every translation names the file as xinput9_1_0.dll.
            ui.add(egui::Label::new(self.i18n.t("rs_linux_help").replace(proxy::MAIN, name)).wrap());
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Label::new(egui::RichText::new(&option).monospace().color(pal::TEXT_STRONG)).wrap());
                if ui.add(styled_button(self.i18n.t("rs_copy"), Look::Secondary)).clicked() {
                    ui.ctx().copy_text(option.clone());
                    self.failed = false;
                    self.message = self.i18n.t("rs_copied");
                    self.message_detail.clear();
                    self.message_area = Area::Mods;
                }
            });
        });
        ui.add_space(st::S3);
    }

    fn refresh(&mut self) {
        self.rows = rows_from_disk(&self.root);
        self.side_bar_old = mods::side_bar_outdated(&self.root);
        self.loaders = proxy::installed(&self.root);
    }

    /// Two NMS.rs loader files in Binaries (say xinput9_1_0.dll and winmm.dll): only the first to
    /// load runs the mods, the other only passes calls through.
    fn loader_note(&self, ui: &mut egui::Ui) {
        if self.loaders.len() < 2 {
            return;
        }
        let text = self.i18n.t("rs_loader_twice").replace("{files}", &self.loaders.join(", "));
        ui.add(egui::Label::new(egui::RichText::new(text).color(pal::WARN).strong()).wrap());
        ui.add_space(st::S3);
    }

    /// "Press Apply sizes ..." while the side bar is an old layout (update keeps size-built files).
    fn side_bar_note(&self, ui: &mut egui::Ui) {
        if !self.side_bar_old {
            return;
        }
        let text = self.i18n.t("rs_side_bar_old").replace("{button}", &self.i18n.t("rs_apply"));
        ui.add(egui::Label::new(egui::RichText::new(text).color(pal::WARN).strong()).wrap());
    }

    fn set_result(&mut self, result: Result<String, String>) {
        self.failed = result.is_err();
        let ok = result.is_ok();
        let text = result.unwrap_or_else(|e| e);
        match text.split_once(DETAIL_SEP) {
            Some((short, detail)) => {
                self.message = short.to_string();
                self.message_detail = detail.to_string();
            }
            None => {
                self.message = text;
                self.message_detail.clear();
            }
        }
        if ok {
            self.refresh();
        }
    }

    fn play(&mut self, ctx: &egui::Context) {
        self.message_area = Area::Play;
        let steam = store::kind(&self.root) != Store::Gog;
        let result = if steam {
            // Steam counts any program running from the game folder as the game, so it answers
            // "already running" while this window is open. The window closes right after the
            // steam:// link is handed to Explorer; Steam gets the link after the manager is gone.
            spawn::gui("explorer.exe")
                .arg(STEAM_PLAY)
                .spawn()
                .map(|_| format!("asked Steam to start the game ({STEAM_PLAY})"))
                .map_err(|e| format!("Could not start the game: {e}"))
        } else {
            let exe = self.root.join("Binaries").join("NMS.exe");
            spawn::gui(&exe).spawn().map(|_| format!("started {}", exe.display())).map_err(|e| format!("Could not start {}: {e}", exe.display()))
        };
        if let Err(text) = &result {
            log_error(&self.root, &format!("play failed: {text}"));
        }
        let started = result.is_ok();
        self.set_result(result);
        if started && steam {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

/// The four button styles of the window. Every button (toolbar, cards, Settings, migration
/// panel) uses one of them: same radius, same height.
#[derive(Clone, Copy)]
enum Look {
    /// Blue fill: the main action (Install, Apply, Pack logs, Migrate).
    Primary,
    /// Red outline: destructive (Remove, Remove all, clean-up).
    Danger,
    /// Amber fill: something needs a look but is not bad (Update).
    Attention,
    /// Neutral raised surface (Not now, Close, Reset).
    Secondary,
}

fn button_parts(look: Look) -> (Color32, egui::Stroke, Color32) {
    match look {
        Look::Primary => (pal::PRIMARY, egui::Stroke::new(1.0_f32, pal::PRIMARY_LIGHT), pal::TEXT_STRONG),
        Look::Danger => (Color32::TRANSPARENT, egui::Stroke::new(1.0_f32, pal::ACCENT_RED), pal::ACCENT_RED_LIGHT),
        Look::Attention => (pal::ATTENTION, egui::Stroke::new(1.0_f32, pal::ATTENTION_LIGHT), pal::TEXT_ON_ATTENTION),
        Look::Secondary => (pal::SURFACE_RAISED, egui::Stroke::new(1.0_f32, pal::EDGE), pal::TEXT),
    }
}

/// A button in one of the window's looks with hover and press feedback. egui's `Button::fill`
/// replaces its own hover shading, so the background is painted here instead: lighter on
/// hover, darker while pressed (outlined looks get a tinted background and a brighter edge),
/// eased over about 120 ms. The animation only repaints while it runs.
struct StyledButton {
    text: egui::RichText,
    fill: Color32,
    stroke: egui::Stroke,
    min_size: egui::Vec2,
    wrap: egui::TextWrapMode,
}

impl StyledButton {
    fn new(text: egui::RichText, fill: Color32, stroke: egui::Stroke) -> Self {
        Self { text, fill, stroke, min_size: egui::vec2(0.0, st::CONTROL_H), wrap: egui::TextWrapMode::Extend }
    }

    fn min_size(mut self, size: egui::Vec2) -> Self {
        self.min_size = size;
        self
    }

    fn wrap(mut self) -> Self {
        self.wrap = egui::TextWrapMode::Wrap;
        self
    }
}

/// Background and edge of a button `hover` (0..=1) and `press` (0..=1) of the way in.
fn button_feedback(fill: Color32, stroke: egui::Stroke, hover: f32, press: f32) -> (Color32, egui::Stroke) {
    let outlined = fill.a() == 0;
    if outlined {
        let tint = stroke.color.gamma_multiply(0.10 * hover + 0.12 * press);
        let edge = pal::mix(stroke.color, pal::HOVER_TOWARD, 0.25 * hover);
        (tint, egui::Stroke::new(stroke.width, edge))
    } else {
        let lit = pal::mix(fill, pal::HOVER_TOWARD, 0.12 * hover);
        let fill = pal::mix(lit, pal::PRESS_TOWARD, 0.18 * press);
        let edge = pal::mix(stroke.color, pal::HOVER_TOWARD, 0.20 * hover);
        (fill, egui::Stroke::new(stroke.width, edge))
    }
}

impl egui::Widget for StyledButton {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        // Reserve the background slot first so it draws under the label.
        let bg = ui.painter().add(egui::Shape::Noop);
        let button = egui::Button::new(self.text)
            .fill(Color32::TRANSPARENT)
            .stroke(egui::Stroke::NONE)
            .corner_radius(st::RADIUS_CONTROL)
            .min_size(self.min_size)
            .wrap_mode(self.wrap);
        let mut response = ui.add(button);
        let active = response.enabled();
        let hovered = active && response.hovered();
        let pressed = active && response.is_pointer_button_down_on();
        let ctx = ui.ctx().clone();
        let hover = ctx.animate_bool_with_time(response.id.with("hover"), hovered, st::MOTION_FAST);
        let press = ctx.animate_bool_with_time(response.id.with("press"), pressed, 0.08);
        let (fill, stroke) = button_feedback(self.fill, self.stroke, hover, press);
        let shape = egui::epaint::RectShape::new(response.rect, st::RADIUS_CONTROL, fill, stroke, egui::StrokeKind::Inside);
        ui.painter().set(bg, shape);
        if hovered {
            response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        response
    }
}

/// One-line button for short labels (Install, Remove, Update) that sit in rows.
fn styled_button(text: impl Into<String>, look: Look) -> StyledButton {
    let (fill, stroke, color) = button_parts(look);
    StyledButton::new(egui::RichText::new(text.into()).color(color).strong(), fill, stroke)
}

/// Button on its own line whose label can be long in some languages (ko, zh, de, ru, ...):
/// the text wraps at the window edge instead of being cut off.
fn wide_button(text: impl Into<String>, look: Look) -> StyledButton {
    styled_button(text, look).wrap()
}

/// Language dropdown: a button with a caret, and below it (small gap) its own framed list with
/// padding. The list is tall enough for every language when the window allows; otherwise it
/// scrolls with a solid bar beside the rows (never over the highlight). Returns the picked code.
fn language_picker(ui: &mut egui::Ui, names: &[(&'static str, &'static str)], shown: &str, current: &str) -> String {
    const ROW_H: f32 = 26.0;
    const PAD: f32 = st::S1;
    let font = st::font(st::TYPE_BODY);
    let widest = names
        .iter()
        .map(|(_, n)| ui.painter().layout_no_wrap(n.to_string(), font.clone(), pal::TEXT).size().x)
        .fold(0.0_f32, f32::max);
    let width = (widest + st::S3 * 2.0 + 18.0).max(160.0);
    let button = styled_button(shown, Look::Secondary).min_size(egui::vec2(width, st::CONTROL_SM_H));
    let response = ui.add(button);
    // Caret on the right edge of the button.
    let c = egui::pos2(response.rect.right() - st::S3, response.rect.center().y);
    let caret = vec![c + egui::vec2(-4.0, -2.0), c + egui::vec2(4.0, -2.0), c + egui::vec2(0.0, 3.0)];
    ui.painter().add(egui::Shape::convex_polygon(caret, pal::TEXT_MUTED, egui::Stroke::NONE));
    let mut pick = current.to_string();
    let frame = egui::Frame::new()
        .fill(pal::SURFACE_3)
        .stroke(egui::Stroke::new(st::STROKE, pal::EDGE))
        .corner_radius(st::RADIUS_PANEL)
        .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 12, spread: 0, color: pal::SHADOW })
        .inner_margin(egui::Margin::same(PAD as i8));
    let below = (ui.ctx().content_rect().bottom() - response.rect.bottom() - st::S4 - PAD * 2.0).max(ROW_H * 4.0);
    let full = names.len() as f32 * ROW_H;
    egui::Popup::menu(&response).id(egui::Id::new("language_popup")).gap(st::S1).width(width).frame(frame).show(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        egui::ScrollArea::vertical().max_height(full.min(below)).auto_shrink([false, true]).show(ui, |ui| {
            for (code, name) in names {
                let (rect, row) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), egui::Sense::click());
                let selected = *code == current;
                let radius = st::RADIUS_PANEL - PAD as u8;
                if selected {
                    ui.painter().rect_filled(rect, radius, pal::PRIMARY.gamma_multiply(0.45));
                } else if row.hovered() {
                    ui.painter().rect_filled(rect, radius, pal::SURFACE_RAISED);
                }
                let color = if selected { pal::TEXT_STRONG } else { pal::TEXT };
                let galley = ui.painter().layout_no_wrap(name.to_string(), font.clone(), color);
                let pos = egui::pos2(rect.left() + st::S2, rect.center().y - galley.size().y / 2.0);
                ui.painter().galley(pos, galley, color);
                if row.clicked() {
                    pick = code.to_string();
                }
                row.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
        });
    });
    pick
}

/// Separator between the short result (footer text) and its full detail (footer tooltip) in a
/// job's result string; `set_result` splits it.
const DETAIL_SEP: &str = "\u{1f}";

/// Locale words for the clean-up summary (read on the UI thread, used by the job thread).
struct CleanupWords {
    nothing: String,
    python: String,
    steam_none: String,
    steam_removed: String,
}

impl CleanupWords {
    fn new(i18n: &I18n) -> Self {
        Self {
            nothing: i18n.t("rs_cleanup_nothing"),
            python: i18n.t("rs_cleanup_python"),
            steam_none: i18n.t("rs_cleanup_steam_none"),
            steam_removed: i18n.t("rs_cleanup_steam_removed"),
        }
    }
}

/// Short footer line from the clean-up's detail lines (`cleanup_python`, `strip_launch`).
fn cleanup_summary(python: &str, steam: &str, words: &CleanupWords) -> String {
    let deleted = python.lines().filter(|l| l.starts_with("deleted ")).count();
    let accounts: Vec<&str> = steam.lines().filter(|l| l.starts_with("Steam account ")).collect();
    let removed = accounts.iter().filter(|l| l.ends_with("launch option removed.")).count();
    let mut parts = Vec::new();
    if deleted > 0 {
        parts.push(words.python.replace("{n}", &deleted.to_string()));
    } else if removed == 0 {
        parts.push(words.nothing.clone());
    }
    if removed > 0 {
        parts.push(words.steam_removed.replace("{done}", &removed.to_string()).replace("{n}", &accounts.len().to_string()));
    } else if !accounts.is_empty() {
        parts.push(words.steam_none.replace("{n}", &accounts.len().to_string()));
    }
    parts.join(" ")
}

fn dim(text: String) -> egui::RichText {
    egui::RichText::new(text).color(pal::TEXT_MUTED).small()
}

/// A setting or notice panel: elevation 2, the one border, the panel radius and padding.
fn panel_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(pal::SURFACE_2)
        .stroke(st::border())
        .corner_radius(st::RADIUS_PANEL)
        .inner_margin(egui::Margin::same(st::S4 as i8))
}

/// Section title inside a panel (type scale: heading).
fn section_title(text: String) -> egui::RichText {
    egui::RichText::new(text).size(st::TYPE_HEADING).color(pal::PRIMARY_LIGHT)
}

impl Manager {
    /// Dot colour of a card: amber when an update is waiting, else green (live) or grey (off).
    fn card_dot(row: &ModRow) -> Color32 {
        if has_update(row) {
            pal::ATTENTION
        } else if row.state == "live" {
            pal::SUCCESS
        } else {
            pal::PARKED
        }
    }

    fn run_card_action(&mut self, name: &str, action: CardAction) {
        let name = name.to_string();
        match action {
            CardAction::Install => {
                let label = format!("{} {}", self.i18n.t("install"), name);
                self.start_job(Area::Mods, label, move |root| mods::install_mod(root, &name, files::game_is_running()));
            }
            CardAction::Park => {
                let label = format!("{} {}", self.i18n.t("remove"), name);
                self.start_job(Area::Mods, label, move |root| mods::park_mod(root, &name, files::game_is_running()));
            }
            CardAction::Update => {
                let label = format!("{} {}", self.i18n.t("update"), name);
                self.start_job(Area::Mods, label, move |root| mods::update_mod(root, &name, files::game_is_running()));
            }
        }
    }

    /// One mod as a card in `rect`: hex badge with a state dot, the name, up to three lines of
    /// description (full text on hover), and the footer row (state word, Update when outdated, the
    /// on/off switch) pinned to the bottom edge.
    ///
    /// Nothing here is measured: `rect` and `text` were laid out before, in this frame.
    fn mod_card(&mut self, ui: &mut egui::Ui, row: &ModRow, rect: egui::Rect, text: cards::CardText, running: bool) {
        let ctx = ui.ctx().clone();
        let live = row.state == "live";
        let update = has_update(row);
        let id = ui.id().with(("card", &row.name));
        let response = ui.interact(rect, id, egui::Sense::hover());
        let hover = ctx.animate_bool_with_time(id.with("hover"), response.contains_pointer(), st::MOTION_FAST);
        let painter = ui.painter().clone();
        let radius = egui::CornerRadius::same(st::RADIUS_PANEL);
        // Hover lifts the card one elevation level: lighter surface, accent edge, soft shadow.
        if hover > 0.0 {
            painter.rect_filled(rect.translate(egui::vec2(0.0, 2.0)), radius, pal::SHADOW.gamma_multiply(hover));
        }
        let (edge_idle, edge_hot) = if update { (pal::ATTENTION.gamma_multiply(0.7), pal::ATTENTION) } else { (pal::EDGE, pal::PRIMARY) };
        let edge = st::lerp_color(edge_idle, edge_hot, hover);
        painter.rect(rect, radius, st::elevation(2.0 + hover), egui::Stroke::new(st::STROKE, edge), egui::StrokeKind::Inside);
        let mut card_ui = ui.new_child(egui::UiBuilder::new().id_salt(("card_ui", &row.name)).max_rect(rect));
        card_ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        let pad = cards::CARD_PAD;
        let inner = rect.shrink(pad);
        let foot_mid = inner.bottom() - st::CONTROL_H / 2.0;
        // Text and actions stay above the footer row while the card grows or shrinks.
        let body_clip = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), foot_mid - st::CONTROL_H / 2.0 - st::S1)).intersect(card_ui.clip_rect());
        let body = painter.with_clip_rect(body_clip);
        cards::paint_badge(&body, inner.min, &row.name, cards::group_of(&row.name).color());
        // State dot on the badge corner: green live, grey parked, amber update available.
        cards::paint_dot(&body, inner.min + egui::vec2(cards::BADGE - 4.0, 6.0), 4.5, Self::card_dot(row));
        let title_pos = egui::pos2(inner.left() + cards::BADGE + st::S3, inner.top() + text.title_top);
        let title_rect = egui::Rect::from_min_size(title_pos, text.title.size());
        let title_elided = text.title.elided;
        body.galley(title_pos, text.title, pal::TEXT_STRONG);
        let title_tip = if title_elided { format!("{}\n{}", cards::pretty_name(&row.name), row.name) } else { row.name.clone() };
        card_ui.interact(title_rect, id.with("title"), egui::Sense::hover()).on_hover_text(title_tip);
        let desc_pos = egui::pos2(inner.left(), inner.top() + text.head_h + st::S2);
        let desc_rect = egui::Rect::from_min_size(desc_pos, text.desc.size());
        let desc_elided = text.desc.elided;
        body.galley(desc_pos, text.desc, pal::TEXT_MUTED);
        if desc_elided {
            card_ui.interact(desc_rect, id.with("desc"), egui::Sense::hover()).on_hover_text(self.i18n.mod_description(&row.name));
        }
        let mut action = None;
        // Footer row: everything centred on one midline.
        if !update {
            let (word, color) = match row.state.as_str() {
                "live" => (self.i18n.t("rs_live"), pal::SUCCESS),
                "parked" => (self.i18n.t("parked"), pal::PARKED),
                _ => (self.i18n.t("rs_not_installed"), pal::PARKED),
            };
            let w = (inner.width() - cards::SWITCH.x - st::S2).max(20.0);
            let g = st::galley(&painter, word, st::TYPE_CAPTION, w, 1);
            st::text_mid(&painter, g, inner.left(), foot_mid, color);
        }
        let sw = egui::Rect::from_center_size(egui::pos2(inner.right() - cards::SWITCH.x / 2.0, foot_mid), cards::SWITCH);
        let tip = self.i18n.t(if live { "remove" } else { "install" });
        if cards::switch(&mut card_ui, id.with("switch"), sw, live, !running).on_hover_text(tip).clicked() {
            action = Some(if live { CardAction::Park } else { CardAction::Install });
        }
        if update {
            let label = self.i18n.t("update");
            let w = st::button_width(&card_ui, &label);
            let b = egui::Rect::from_min_size(egui::pos2(sw.left() - st::S2 - w, foot_mid - st::CONTROL_SM_H / 2.0), egui::vec2(w, st::CONTROL_SM_H));
            if st::place(&mut card_ui, "update", b, !running, styled_button(label, Look::Attention)).clicked() {
                action = Some(CardAction::Update);
            }
        }
        self.layout.push((format!("card:{}", row.name), rect));
        if let Some(action) = action {
            self.run_card_action(&row.name, action);
        }
    }

    /// One row of cards, all the same height. Every card's title and description are laid out
    /// first (same wrap width and line caps as the painting), the row takes the tallest, then all
    /// cards are painted at that height: the same frame, nothing stored, no settling frame.
    fn card_row(&mut self, ui: &mut egui::Ui, items: &[ModRow], spans: &[(f32, f32)], running: bool) {
        if items.is_empty() {
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let painter = ui.painter().clone();
        let top = ui.cursor().top();
        let mut texts = Vec::with_capacity(items.len());
        let mut height = 0.0_f32;
        for (row, (l, r)) in items.iter().zip(spans) {
            let title = cards::pretty_name(&row.name);
            let desc = self.i18n.mod_description(&row.name);
            let text = cards::CardText::new(&painter, &title, &desc, r - l);
            height = height.max(text.closed_height());
            texts.push(text);
        }
        let height = st::snap(ppp, height);
        for ((row, (l, r)), text) in items.iter().zip(spans).zip(texts) {
            let rect = egui::Rect::from_min_max(egui::pos2(*l, top), egui::pos2(*r, top + height));
            self.mod_card(ui, row, rect, text, running);
        }
        let all = egui::Rect::from_min_max(egui::pos2(spans[0].0, top), egui::pos2(spans[items.len() - 1].1, top + height));
        ui.advance_cursor_after_rect(all);
        ui.add_space(cards::CARD_GAP);
    }

    /// Cards under one heading per type, filtered by the sidebar filter and the search box. An
    /// open card (and one still closing) takes a full row of its own. The column count has
    /// hysteresis (`cards::columns_stable`), so it does not flip at the breakpoint.
    fn card_grid(&mut self, ui: &mut egui::Ui, running: bool) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let saved = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        let left = ui.max_rect().left();
        let width = ui.available_width();
        let cols = cards::columns_stable(width, self.grid_cols);
        self.grid_cols = Some(cols);
        let spans = cards::grid_columns(left, width, cols, ppp);
        let mut any = false;
        for group in cards::GROUPS {
            let items: Vec<ModRow> = self
                .rows
                .iter()
                .filter(|row| cards::group_of(&row.name) == group && self.filter.matches(row))
                .filter(|row| cards::matches_search(&row.name, &self.i18n.mod_description(&row.name), &self.search))
                .cloned()
                .collect();
            if items.is_empty() {
                continue;
            }
            if any {
                ui.add_space(st::S3);
            }
            any = true;
            self.group_heading(ui, group, items.len(), left, width);
            for row in items.chunks(cols) {
                self.card_row(ui, row, &spans, running);
            }
        }
        if !any {
            ui.add_space(st::S2);
            ui.label(dim(self.i18n.t("rs_no_match")));
        }
        ui.spacing_mut().item_spacing.y = saved;
    }

    /// Group heading: a dot in the group colour on the cards' left edge, the name and the count,
    /// all centred on one midline.
    fn group_heading(&mut self, ui: &mut egui::Ui, group: cards::Group, n: usize, left: f32, width: f32) {
        let top = ui.cursor().top();
        let rect = egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, GROUP_HEADING_H));
        let painter = ui.painter().clone();
        let mid = rect.center().y;
        cards::paint_dot(&painter, egui::pos2(left + 4.0, mid), 4.0, group.color());
        let title = st::galley(&painter, self.i18n.t(group.key()), st::TYPE_HEADING, width, 1);
        let title_rect = st::text_mid(&painter, title, left + st::S4, mid, group.color().lerp_to_gamma(pal::TEXT_STRONG, 0.4));
        let count = st::galley(&painter, format!("{n}"), st::TYPE_CAPTION, width, 1);
        st::text_mid(&painter, count, title_rect.right() + st::S2, mid, pal::TEXT_MUTED);
        self.layout.push((format!("heading:{}", group.key()), rect));
        ui.advance_cursor_after_rect(rect);
    }

    /// The four filters with their counts as `NavEntry`s.
    fn filter_entries(&self) -> Vec<cards::NavEntry> {
        FILTERS
            .iter()
            .map(|f| {
                let n = cards::count(&self.rows, *f);
                // Update with something waiting is amber until selected.
                let idle = (*f == Filter::Update && n > 0).then_some(pal::ATTENTION_LIGHT);
                cards::NavEntry { label: format!("{} ({})", self.i18n.t(f.key()), n), idle }
            })
            .collect()
    }

    fn filter_index(&self) -> usize {
        FILTERS.iter().position(|f| *f == self.filter).unwrap_or(0)
    }

    /// Where the toolbar puts the search box, the bulk buttons and "?" for a window `width`
    /// points wide, and how tall it is. Pure layout from text sizes, used before painting.
    fn toolbar_layout(&self, ui: &egui::Ui, screen: egui::Rect, wide: bool) -> ToolbarLayout {
        let right = screen.right() - st::GUTTER;
        let help = egui::Rect::from_min_size(egui::pos2(right - st::CONTROL_H, screen.top()), egui::vec2(st::CONTROL_H, st::CONTROL_H));
        let mut buttons: Vec<(String, Look, Bulk)> = vec![
            (self.i18n.t("install_all"), Look::Primary, Bulk::Install),
            (self.i18n.t("remove_all"), Look::Danger, Bulk::Remove),
        ];
        // Same check and same file work as the per-card Update buttons and `mods update-all`
        // (a user-edited file newer than stock is not counted and not overwritten).
        if let Some(label) = update_all_label(&self.i18n.t("update_all"), &self.rows) {
            buttons.push((label, Look::Attention, Bulk::Update));
        }
        let mut widths: Vec<f32> = buttons.iter().map(|(l, _, _)| st::button_width(ui, l)).collect();
        // Wide: the search box fills the sidebar column and the buttons start on the content
        // gutter. Narrow: the search box leads the same row.
        let (search_x, search_w, start) = if wide {
            (screen.left() + st::GUTTER, st::SIDEBAR_W - 2.0 * st::GUTTER, screen.left() + st::SIDEBAR_W + st::GUTTER)
        } else {
            let w = (right - screen.left() - st::GUTTER - st::CONTROL_H - st::S2).clamp(80.0, 180.0);
            (screen.left() + st::GUTTER, w, screen.left() + st::GUTTER)
        };
        if !wide {
            widths.insert(0, search_w);
        }
        let avail = (help.left() - st::S2 - start).max(60.0);
        let (pos, rows) = st::flow(&widths, avail, st::S2);
        let mut rects: Vec<egui::Rect> = pos
            .iter()
            .zip(&widths)
            .map(|((r, x), w)| egui::Rect::from_min_size(egui::pos2(start + x, screen.top() + *r as f32 * (st::CONTROL_H + st::S2)), egui::vec2(*w, st::CONTROL_H)))
            .collect();
        let search = if wide {
            egui::Rect::from_min_size(egui::pos2(search_x, screen.top()), egui::vec2(search_w, st::CONTROL_H))
        } else {
            rects.remove(0)
        };
        ToolbarLayout {
            search,
            buttons: buttons.into_iter().zip(rects).map(|((l, look, b), r)| (l, look, b, r)).collect(),
            help,
            height: st::S3 + st::rows_height(rows, st::CONTROL_H, st::S2) + st::S3,
        }
    }

    /// The toolbar row(s) at `top`: search box, Install all, Remove all, Update all (n), "?".
    fn toolbar(&mut self, ui: &mut egui::Ui, layout: ToolbarLayout, running: bool) {
        let ToolbarLayout { search, buttons, help, .. } = layout;
        let edit = egui::TextEdit::singleline(&mut self.search)
            .hint_text(self.i18n.t("rs_search"))
            .margin(egui::Margin::symmetric(cards::FIELD_PAD_X as i8, 0))
            .vertical_align(egui::Align::Center)
            .desired_width(search.width())
            .min_size(search.size());
        st::place(ui, "search", search, true, edit);
        self.layout.push(("search".into(), search));
        for (i, (label, look, bulk, rect)) in buttons.into_iter().enumerate() {
            self.layout.push((format!("button:{i}"), rect));
            if st::place(ui, ("bulk", i), rect, !running, styled_button(label.clone(), look)).clicked() {
                match bulk {
                    Bulk::Install => self.start_job(Area::Mods, label, |root| mods::install_all(root, files::game_is_running())),
                    Bulk::Remove => self.start_job(Area::Mods, label, |root| mods::park_all(root, files::game_is_running())),
                    Bulk::Update => self.start_job(Area::Mods, label, |root| mods::update_all(root, files::game_is_running())),
                }
            }
        }
        self.layout.push(("help".into(), help));
        let response = st::place(ui, "help", help, true, styled_button("?", Look::Secondary)).on_hover_text(self.i18n.t("rs_help_tip"));
        if response.clicked() {
            self.help_open = !self.help_open;
        }
        self.help_popover(ui.ctx(), help, response.clicked());
    }

    /// The help paragraph and the legend under the "?" button until the next click elsewhere.
    /// Fades in and out.
    fn help_popover(&mut self, ctx: &egui::Context, anchor: egui::Rect, anchor_clicked: bool) {
        let t = ctx.animate_bool_with_time(egui::Id::new("help_fade"), self.help_open, st::MOTION);
        if t <= 0.0 {
            return;
        }
        let max_w = (ctx.content_rect().width() - 2.0 * st::GUTTER).clamp(200.0, 420.0);
        let area = egui::Area::new(egui::Id::new("help_popover"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(anchor.right_bottom() + egui::vec2(0.0, st::S1 + st::S1 * (1.0 - t)))
            .show(ctx, |ui| {
                ui.multiply_opacity(t);
                egui::Frame::popup(ui.style())
                    .fill(pal::SURFACE_2)
                    .stroke(egui::Stroke::new(st::STROKE, pal::PRIMARY))
                    .corner_radius(st::RADIUS_PANEL)
                    .inner_margin(egui::Margin::same(st::S3 as i8))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.add(egui::Label::new(self.i18n.t("rs_mods_help")).wrap());
                        ui.add_space(st::S1);
                        for (key, color) in [("rs_live", pal::SUCCESS), ("parked", pal::PARKED), ("rs_outdated", pal::ATTENTION)] {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
                            cards::paint_dot(ui.painter(), egui::pos2(rect.left() + 4.0, rect.center().y), 4.0, color);
                            let g = st::galley(ui.painter(), self.i18n.t(key), st::TYPE_BODY, rect.width() - st::S4, 1);
                            st::text_mid(ui.painter(), g, rect.left() + st::S4, rect.center().y, color);
                        }
                    });
            });
        if self.help_open && area.response.clicked_elsewhere() && !anchor_clicked {
            self.help_open = false;
        }
    }

    /// The scrolling page of the Mods tab: notices, the card grid and Sizes.
    fn mods_page(&mut self, ui: &mut egui::Ui, running: bool) {
        self.legacy_panel(ui, running);
        self.linux_note(ui);
        self.loader_note(ui);
        self.side_bar_note(ui);
        self.card_grid(ui, running);
        ui.add_space(st::S2);
        self.sizes_section(ui, running);
    }

    fn sizes_section(&mut self, ui: &mut egui::Ui, running: bool) {
        let title = format!("{}   {}", self.i18n.t("sizes"), self.i18n.t("rs_click_to_open"));
        panel_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        egui::CollapsingHeader::new(section_title(title))
            .id_salt("sizes_section")
            .default_open(false)
            .open(self.sizes_open.then_some(true))
            .show(ui, |ui| {
                ui.add(egui::Label::new(dim(self.i18n.t("sizes_help"))).wrap());
                ui.add_space(4.0);
                egui::Grid::new("sizes").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
                    ui.label("");
                    ui.add(egui::Label::new(dim(self.i18n.t("sizes_grid_blurb"))).wrap());
                    ui.end_row();
                    ui.label(self.i18n.t("columns"));
                    ui.text_edit_singleline(&mut self.sizes.columns);
                    ui.end_row();
                    ui.label(self.i18n.t("rows"));
                    ui.text_edit_singleline(&mut self.sizes.rows);
                    ui.end_row();
                    ui.label("");
                    ui.label(dim(self.i18n.t("rows_hint")));
                    ui.end_row();
                    ui.label(self.i18n.t("window_scale"));
                    ui.text_edit_singleline(&mut self.sizes.window);
                    ui.end_row();
                    ui.label("");
                    ui.label(dim(self.i18n.t("window_scale_hint")));
                    ui.end_row();
                    ui.label("");
                    ui.add(egui::Label::new(dim(self.i18n.t("sizes_slots_blurb"))).wrap());
                    ui.end_row();
                    ui.label(self.i18n.t("slot_size"));
                    ui.text_edit_singleline(&mut self.sizes.slots);
                    ui.end_row();
                    ui.label(self.i18n.t("filter_buttons"));
                    ui.checkbox(&mut self.sizes.filters, self.i18n.t("filter_buttons_hint"));
                    ui.end_row();
                });
                self.side_bar_note(ui);
                let apply = wide_button(self.i18n.t("rs_apply"), Look::Primary);
                if ui.add_enabled(!running, apply).clicked() {
                    self.sizes = self.sizes.clamp();
                    let spec = self.sizes.to_spec();
                    let label = self.i18n.t("rs_apply");
                    self.start_job(Area::Sizes, label, move |root| apply::apply(root, spec, files::game_is_running()));
                }
            });
        ui.add(egui::Label::new(dim(self.i18n.t("sizes_summary"))).wrap());
        });
    }

}

impl egui_software_backend::App for Manager {
    fn ui(&mut self, root_ui: &mut egui::Ui, _backend: &mut SoftwareBackend) {
        self.draw(root_ui);
    }
}

impl Manager {
    /// One frame of the window (also used by the headless screenshot test).
    ///
    /// The shell is one connected surface, laid out by hand from the window rect: the top chrome
    /// (header band, tabs, toolbar and, when narrow, the filter strip) with a divider under the
    /// tabs and one under the chrome; the sidebar and the page share the body with one vertical
    /// divider; the footer sits on the bottom edge with a divider on top. Every block starts on
    /// the same gutter (`style::GUTTER`) and ends on the same right edge.
    fn draw(&mut self, root_ui: &mut egui::Ui) {
        let ctx = root_ui.ctx().clone();
        let ctx = &ctx;
        self.sync_zoom(ctx);
        self.sync_fonts(ctx);
        let stale = self.running_checked.is_none_or(|t| t.elapsed() >= RUNNING_CHECK_EVERY);
        if stale {
            self.running = files::game_is_running();
            self.running_checked = Some(std::time::Instant::now());
            self.recheck_py_open();
        }
        ctx.request_repaint_after(RUNNING_CHECK_EVERY);
        self.poll_job();
        self.poll_detect();
        if self.job.is_some() || self.legacy_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        }
        let working = self.job.is_some();
        let running = self.running || working;
        if !self.theme_set {
            cards::apply_theme(ctx);
            self.theme_set = true;
        }
        let logo = self.logo.get_or_insert_with(|| {
            let image = egui::ColorImage::from_rgba_unmultiplied([ICON_SIZE, ICON_SIZE], ICON_RGBA);
            ctx.load_texture("nms-rs-logo", image, egui::TextureOptions::LINEAR)
        });
        let logo = logo.id();
        self.layout.clear();
        let ppp = ctx.pixels_per_point();
        let screen = st::snap_rect(ppp, root_ui.max_rect());
        let painter = root_ui.painter().clone();
        painter.rect_filled(screen, 0, pal::SURFACE_0);
        let mods_tab = self.tab == Tab::Mods;
        let wide = screen.width() >= st::SIDEBAR_MIN_WIDTH;
        let (left, right) = (screen.left(), screen.right());
        let row = |top: f32, h: f32| egui::Rect::from_x_y_ranges(left..=right, top..=top + h);

        // Top chrome: every height is known before anything is drawn.
        let header = row(screen.top(), st::HEADER_H);
        let tabs = row(header.bottom(), st::TABS_H);
        let mut chrome_bottom = tabs.bottom();
        let toolbar = mods_tab.then(|| {
            let probe = egui::Rect::from_x_y_ranges(left..=right, chrome_bottom + st::S3..=chrome_bottom + st::S3);
            let layout = self.toolbar_layout(root_ui, probe, wide);
            let rect = row(chrome_bottom, layout.height);
            chrome_bottom = rect.bottom();
            (rect, layout)
        });
        let entries = self.filter_entries();
        let strip = (mods_tab && !wide).then(|| {
            let inner_w = screen.width() - 2.0 * st::GUTTER;
            let h = cards::underline_nav_height(&painter, &entries, STRIP_NAV, inner_w);
            // Starts inside the toolbar's bottom padding; the underline sits on the chrome divider.
            let rect = row(chrome_bottom - st::S2, h);
            chrome_bottom = rect.bottom();
            rect
        });
        let chrome = egui::Rect::from_x_y_ranges(left..=right, screen.top()..=chrome_bottom);
        let footer = row(screen.bottom() - st::FOOTER_H, st::FOOTER_H);
        let body = egui::Rect::from_x_y_ranges(left..=right, chrome.bottom()..=footer.top());
        let sidebar = (mods_tab && wide).then(|| egui::Rect::from_x_y_ranges(left..=left + st::SIDEBAR_W, body.y_range()));
        let content = egui::Rect::from_x_y_ranges(sidebar.map_or(left, |s| s.right())..=right, body.y_range());
        for (name, rect) in [("header", header), ("tabs", tabs), ("chrome", chrome), ("body", body), ("content", content), ("footer", footer)] {
            self.layout.push((name.into(), rect));
        }

        // Surfaces: chrome and sidebar are elevation 1, the page elevation 0.
        painter.rect_filled(chrome, 0, pal::SURFACE_1);
        painter.add(cards::gradient_shape(header, pal::HEADER_TOP, pal::HEADER_BOTTOM));
        if let Some(side) = sidebar {
            painter.rect_filled(side, 0, pal::SURFACE_1);
            st::vline(&painter, side.right() - 0.5, side.top(), side.bottom());
            self.layout.push(("sidebar".into(), side));
        }
        st::hline(&painter, left, right, tabs.bottom() - 0.5);
        st::hline(&painter, left, right, chrome.bottom() - 0.5);

        // Header band: logo, name and version on the left gutter, Play on the right edge.
        let mid = header.center().y;
        let logo_rect = egui::Rect::from_min_size(egui::pos2(left + st::GUTTER, mid - 18.0), egui::vec2(36.0, 36.0));
        painter.image(logo, logo_rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        self.layout.push(("logo".into(), logo_rect));
        let title = st::galley(&painter, "NMS.rs", st::TYPE_TITLE, f32::INFINITY, 0);
        let title_baseline = baseline(&title);
        let title_rect = st::text_mid(&painter, title, logo_rect.right() + st::S3, mid, pal::TEXT_STRONG);
        let version = st::galley(&painter, format!("v{}", nms_core::ipc::VERSION), st::TYPE_BODY, f32::INFINITY, 0);
        // The version sits on the title's baseline.
        let version_pos = egui::pos2(title_rect.right() + st::S2, title_rect.top() + title_baseline - baseline(&version));
        painter.galley(version_pos, version, pal::PRIMARY_LIGHT);
        let play_w = st::button_width(root_ui, &self.i18n.t("play")).max(112.0) + st::S4;
        let play_rect = egui::Rect::from_min_size(egui::pos2(right - st::GUTTER - play_w, mid - 18.0), egui::vec2(play_w, 36.0));
        self.layout.push(("play".into(), play_rect));
        let label = egui::RichText::new(self.i18n.t("play")).size(st::TYPE_TAB + 2.0).color(pal::TEXT_ON_PLAY);
        let play = StyledButton::new(label, pal::PLAY, egui::Stroke::new(st::STROKE, pal::PLAY_LIGHT)).min_size(egui::Vec2::ZERO);
        if st::place(root_ui, "play", play_rect, true, play).clicked() {
            let ctx = root_ui.ctx().clone();
            self.play(&ctx);
        }

        // Tabs on the chrome's first divider, text starting on the gutter.
        let tab_entries: Vec<cards::NavEntry> = [(Tab::Mods, "tab_mods"), (Tab::Settings, "rs_settings")]
            .iter()
            .map(|(_, key)| cards::NavEntry { label: self.i18n.t(key), idle: None })
            .collect();
        let tab_area = egui::Rect::from_x_y_ranges(left + st::GUTTER..=right - st::GUTTER, tabs.y_range());
        let selected = if mods_tab { 0 } else { 1 };
        if let Some(i) = cards::underline_nav(root_ui, "tabs", tab_area, &tab_entries, selected, TAB_NAV) {
            self.tab = if i == 0 { Tab::Mods } else { Tab::Settings };
        }

        if let Some((rect, layout)) = toolbar {
            self.layout.push(("toolbar".into(), rect));
            self.toolbar(root_ui, layout, running);
        }
        if let Some(rect) = strip {
            let area = egui::Rect::from_x_y_ranges(left + st::GUTTER..=right - st::GUTTER, rect.y_range());
            if let Some(i) = cards::underline_nav(root_ui, "filter_strip", area, &entries, self.filter_index(), STRIP_NAV) {
                self.filter = FILTERS[i];
            }
        }
        if let Some(side) = sidebar {
            let x = egui::Rangef::new(side.left() + st::GUTTER, side.right() - st::GUTTER);
            if let Some(i) = cards::side_list(root_ui, "filters", x, side.top() + st::S3, &entries, self.filter_index(), st::TYPE_BODY) {
                self.filter = FILTERS[i];
            }
        }

        // The page: gutters inside the scroll area; the floating scroll bar takes no width.
        let mut page = root_ui.new_child(egui::UiBuilder::new().id_salt("page_ui").max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)));
        page.set_clip_rect(content);
        egui::ScrollArea::vertical().id_salt("page").auto_shrink([false, false]).show(&mut page, |ui| {
            let g = st::GUTTER as i8;
            egui::Frame::new().inner_margin(egui::Margin { left: g, right: g, top: st::S4 as i8, bottom: st::S4 as i8 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                match self.tab {
                    Tab::Mods => self.mods_page(ui, running),
                    Tab::Settings => self.settings_section(ui, ctx, running),
                }
            });
        });

        self.footer(root_ui, footer);
    }
}

/// Baseline of the first line of `g`, from its top.
fn baseline(g: &egui::Galley) -> f32 {
    g.rows.first().and_then(|r| r.row.glyphs.first().map(|gl| r.pos.y + gl.pos.y)).unwrap_or(g.size().y)
}

/// The tab bar and the narrow filter strip.
const TAB_NAV: cards::NavStyle = cards::NavStyle { size: st::TYPE_TAB, gap: st::S5, row_h: st::TABS_H };
const STRIP_NAV: cards::NavStyle = cards::NavStyle { size: st::TYPE_BODY, gap: st::S4, row_h: st::CONTROL_H };

/// Height of a group heading row in the card grid.
const GROUP_HEADING_H: f32 = 32.0;

/// Box of the footer's status dot or spinner.
const FOOTER_MARKER: f32 = 12.0;

/// A toolbar bulk action.
#[derive(Clone, Copy)]
enum Bulk {
    Install,
    Remove,
    Update,
}

/// Rects of the toolbar, computed from text sizes before painting (`Manager::toolbar_layout`).
struct ToolbarLayout {
    search: egui::Rect,
    buttons: Vec<(String, Look, Bulk, egui::Rect)>,
    help: egui::Rect,
    height: f32,
}

/// True when this Windows exe runs under Wine or Proton (ntdll exports `wine_get_version`).
#[cfg(windows)]
fn is_wine() -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleA(name: *const u8) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
    }
    // SAFETY: both names are NUL-terminated literals; a null module handle is checked.
    unsafe {
        let ntdll = GetModuleHandleA(b"ntdll.dll\0".as_ptr());
        !ntdll.is_null() && !GetProcAddress(ntdll, b"wine_get_version\0".as_ptr()).is_null()
    }
}

#[cfg(not(windows))]
fn is_wine() -> bool {
    true
}

#[cfg(windows)]
mod console {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleProcessList(list: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }

    /// Double-clicking nms-rs.exe opens a console that only this process uses; drop it so
    /// only the window stays. Started from a terminal (shared console), leave it alone.
    ///
    /// With no console, console children must come from `nms_core::spawn::command`
    /// (CREATE_NO_WINDOW), or each one opens a window. The old Apply sizes crash was the OpenGL
    /// renderer under overlay hooks, not CREATE_NO_WINDOW; the window is drawn on the CPU now.
    pub fn detach_if_alone() {
        let mut list = [0u32; 2];
        // SAFETY: plain Win32 calls with a valid buffer of the stated length.
        unsafe {
            if GetConsoleProcessList(list.as_mut_ptr(), 2) == 1 {
                FreeConsole();
            }
        }
    }
}

#[cfg(not(windows))]
mod console {
    pub fn detach_if_alone() {}
}

fn log_error(root: &Path, text: &str) {
    let dir = paths::loader_dir(root).join("logs");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("manager.log")) {
        use std::io::Write;
        let _ = writeln!(f, "{text}");
    }
}

/// The window is drawn on the CPU (no OpenGL, Direct3D or Vulkan), so screen-capture and overlay
/// hooks (RTSS, Medal, OBS, Discord) have no graphics API to hook, and it looks the same on
/// every GPU and under Wine/Proton.
pub fn run(root: PathBuf) -> ExitCode {
    console::detach_if_alone();
    let settings = SoftwareBackendAppConfiguration::new()
        .inner_size(Some(egui::vec2(1000.0, 820.0)))
        .min_inner_size(Some(egui::vec2(600.0, 400.0)))
        .title(Some("NMS.rs".to_string()))
        .icon(Some(std::sync::Arc::new(egui::IconData {
            rgba: ICON_RGBA.to_vec(),
            width: ICON_SIZE as u32,
            height: ICON_SIZE as u32,
        })));
    let app_root = root.clone();
    match egui_software_backend::run_app_with_software_backend(settings, move |_ctx| Manager::new(app_root.clone())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            log_error(&root, &format!("manager window failed: {err}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_summary_is_short() {
        let words = CleanupWords::new(&I18n::new("en"));
        let steam = "Steam account 1: already set.\nSteam account 2: already set.\nSteam account 3: already set.";
        assert_eq!(cleanup_summary("no python leftovers", steam, &words), "Nothing to clean up. No old Steam launch option (3 accounts checked).");
        let python = "deleted a.py\ndeleted b.pyc";
        let steam = "Steam account 1: launch option removed.\nSteam account 2: already set.";
        assert_eq!(cleanup_summary(python, steam, &words), "Removed 2 Python files. Steam launch option removed on 1 of 2 accounts.");
    }
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEMP_N: AtomicU32 = AtomicU32::new(0);

    fn temp_root(tag: &str) -> PathBuf {
        let n = TEMP_N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nms-rs-t039-{tag}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("GAMEDATA").join("MODS")).unwrap();
        fs::create_dir_all(paths::loader_dir(&dir).join("stock")).unwrap();
        dir
    }

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn clamps_size_fields() {
        assert_eq!(
            clamp_size_fields("99", "20", "2.05", "110"),
            ("9".into(), "11".into(), "1.4".into(), "100".into())
        );
        assert_eq!(
            clamp_size_fields("8", "auto", "1.4", "70"),
            ("8".into(), "auto".into(), "1.4".into(), "70".into())
        );
        assert_eq!(
            clamp_size_fields("0", "", "0.5", "40"),
            ("1".into(), "auto".into(), "1".into(), "50".into())
        );
        let fields = SizeFields {
            columns: "99".into(),
            rows: "20".into(),
            window: "2.05".into(),
            slots: "110".into(),
            filters: false,
        }
        .clamp();
        assert_eq!(fields.columns, "9");
        assert_eq!(fields.rows, "11");
        assert_eq!(fields.window, "1.4");
        assert_eq!(fields.slots, "100");
        assert!(!fields.filters);
    }

    #[test]
    fn parses_status_lines_into_rows() {
        let text = "QuickTransferAll live\nPulseTweaks missing\npython leftover: yes\nHideEmptyHover parked\n";
        let rows = parse_status_lines(text);
        assert_eq!(
            rows,
            vec![
                ModRow {
                    name: "QuickTransferAll".into(),
                    state: "live".into(),
                    outdated: false
                },
                ModRow {
                    name: "PulseTweaks".into(),
                    state: "missing".into(),
                    outdated: false
                },
                ModRow {
                    name: "HideEmptyHover".into(),
                    state: "parked".into(),
                    outdated: false
                },
            ]
        );
        assert!(!rows.iter().any(|r| r.name.contains("python")));
    }

    #[test]
    fn lists_stock_and_live_our_mod_extras() {
        let root = temp_root("list");
        write(&mods::stock_dir(&root).join("QuickTransferAll").join("a.txt"), "s\n");
        write(&mods::mods_dir(&root).join("CraftFromStorage").join("keep.json"), "{}\n");
        write(&mods::parked_dir(&root).join("HideEmptyHover").join("keep.json"), "{}\n");
        let names = listed_mod_names(&root);
        assert!(names.contains(&"QuickTransferAll".into()), "{names:?}");
        assert!(names.contains(&"CraftFromStorage".into()), "{names:?}");
        assert!(!names.contains(&"HideEmptyHover".into()), "{names:?}");
        let rows = rows_from_disk(&root);
        assert_eq!(rows.iter().find(|r| r.name == "QuickTransferAll").unwrap().state, "missing");
        assert_eq!(rows.iter().find(|r| r.name == "CraftFromStorage").unwrap().state, "live");
        let _ = fs::remove_dir_all(&root);
    }

    fn row(name: &str, state: &str, outdated: bool) -> ModRow {
        ModRow {
            name: name.into(),
            state: state.into(),
            outdated,
        }
    }

    #[test]
    fn update_all_counts_outdated_installed_rows() {
        assert_eq!(outdated_count(&[]), 0);
        assert_eq!(update_all_label("Update all ({n})", &[]), None);
        let fresh = [row("A", "live", false), row("B", "parked", false), row("C", "missing", false)];
        assert_eq!(update_all_label("Update all ({n})", &fresh), None);
        let rows = [
            row("A", "live", true),
            row("B", "parked", true),
            row("C", "live", false),
            row("D", "live", true),
            // Not installed: no folder to update, never counted even if flagged.
            row("E", "missing", true),
        ];
        assert_eq!(outdated_count(&rows), 3);
        assert_eq!(update_all_label("Update all ({n})", &rows).as_deref(), Some("Update all (3)"));
        assert_eq!(update_all_label("Alle aktualisieren ({n})", &rows[..1]).as_deref(), Some("Alle aktualisieren (1)"));
    }

    #[test]
    fn every_locale_update_all_has_count() {
        let mut i = I18n::new("en");
        for (code, _, _) in i18n::codes() {
            i.set(code);
            let label = update_all_label(&i.t("update_all"), &[row("A", "live", true), row("B", "live", true)]).unwrap();
            assert!(label.contains('2') && !label.contains("{n}") && label != "update_all", "{code}: {label}");
        }
    }

    #[test]
    fn update_all_count_follows_disk_and_skips_user_edits() {
        let root = temp_root("update_all");
        // Missing a stock file: out of date.
        write(&mods::stock_dir(&root).join("QuickTransferAll").join("a.lua"), "new\n");
        write(&mods::stock_dir(&root).join("QuickTransferAll").join("b.lua"), "new\n");
        write(&mods::mods_dir(&root).join("QuickTransferAll").join("a.lua"), "new\n");
        // Parked and missing a stock file: out of date too (update-all updates parked mods).
        write(&mods::stock_dir(&root).join("HideEmptyHover").join("a.lua"), "new\n");
        write(&mods::parked_dir(&root).join("HideEmptyHover").join("keep.json"), "{}\n");
        // Edited by the user after the stock copy (newer file): not counted, not overwritten.
        write(&mods::stock_dir(&root).join("CraftFromStorage").join("a.lua"), "stock\n");
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&mods::mods_dir(&root).join("CraftFromStorage").join("a.lua"), "mine\n");
        let rows = rows_from_disk(&root);
        assert_eq!(outdated_count(&rows), 2, "{rows:?}");
        mods::update_all(&root, false).unwrap();
        assert_eq!(outdated_count(&rows_from_disk(&root)), 0);
        let edited = fs::read_to_string(mods::mods_dir(&root).join("CraftFromStorage").join("a.lua")).unwrap();
        assert_eq!(edited, "mine\n");
        assert_eq!(mods::update_all(&root, true).unwrap_err(), mods::BUSY);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn size_fields_from_sizes_json() {
        let text = r#"{
            "BiggerItemGrids": {"columns": 6, "rows": 3, "window_scale": 1.2},
            "SmallerInventorySlots": {"scale": 0.5, "filters": false}
        }"#;
        let fields = size_fields_from_json(text);
        assert_eq!(fields.columns, "6");
        assert_eq!(fields.rows, "3");
        assert_eq!(fields.window, "1.2");
        assert_eq!(fields.slots, "50");
        assert!(!fields.filters);
        let fallback = size_fields_from_json("not json");
        assert_eq!(fallback, SizeFields::defaults());
    }

    /// Minimal PNG writer (stored deflate blocks, no compression) for the screenshot test.
    fn write_png(path: &Path, width: usize, height: usize, rgba: &[[u8; 4]]) {
        fn crc32(data: &[u8]) -> u32 {
            let mut crc = 0xffff_ffffu32;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
                }
            }
            !crc
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
        let mut raw = Vec::with_capacity(height * (width * 4 + 1));
        for row in rgba.chunks(width) {
            raw.push(0);
            for px in row {
                raw.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        let mut z = vec![0x78, 0x01];
        let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
        for (i, block) in blocks.iter().enumerate() {
            z.push(u8::from(i + 1 == blocks.len()));
            let len = block.len() as u16;
            z.extend_from_slice(&len.to_le_bytes());
            z.extend_from_slice(&(!len).to_le_bytes());
            z.extend_from_slice(block);
        }
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in &raw {
            a = (a + byte as u32) % 65_521;
            b = (b + a) % 65_521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(width as u32).to_be_bytes());
        ihdr.extend_from_slice(&(height as u32).to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);
        chunk(&mut png, b"IDAT", &z);
        chunk(&mut png, b"IEND", &[]);
        fs::write(path, png).unwrap();
    }

    /// Headless screenshots of the manager through the same CPU renderer the window uses.
    /// One Manager and one egui context switch through the languages, so this also covers
    /// re-applying fonts on a language change and the glyph atlas updates in the renderer.
    /// Run: `cargo test -p nms-launcher ui_shots -- --ignored`; PNGs go to `NMS_RS_SHOTS`
    /// (default `%TEMP%\nms-rs-shots`).
    #[test]
    #[ignore]
    fn ui_shots() {
        use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
        let out_dir = std::env::var_os("NMS_RS_SHOTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("nms-rs-shots"));
        fs::create_dir_all(&out_dir).unwrap();
        let root = temp_root("shots");
        let mut manager = Manager::new(root.clone());
        // No NMS.py panel here (see `migrate_shots`), whatever this PC's Steam config says.
        manager.legacy_rx = None;
        manager.legacy = Some(Legacy::default());
        // Draw everything that can appear: the Linux notice and the open Sizes section.
        manager.wine = true;
        manager.sizes_open = true;
        // Every mod row with each state, so all buttons and descriptions are drawn.
        manager.rows = mods::OUR_MODS
            .iter()
            .enumerate()
            .map(|(i, name)| ModRow {
                name: (*name).to_string(),
                state: ["live", "parked", "missing"][i % 3].to_string(),
                outdated: i % 3 != 2 && i % 2 == 0,
            })
            .collect();
        // Several outdated installed mods, so the Mods heading row also shows Update all (n).
        assert!(outdated_count(&manager.rows) >= 3, "{:?}", manager.rows);
        let ctx = egui::Context::default();
        let mut renderer = EguiSoftwareRender::new(ColorFieldOrder::Rgba);
        let mut frame = |manager: &mut Manager, picker: bool, w: usize, h: usize, buffer: &mut Vec<[u8; 4]>| {
            buffer.resize(w * h, [0u8; 4]);
            // Like egui-winit: the screen rect is the pixel size over pixels per point (zoom).
            let ppp = ctx.zoom_factor();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / ppp, h as f32 / ppp))),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| {
                manager.draw(ui);
                if picker {
                    let names = manager.picker_names(ui.ctx());
                    egui::Area::new(egui::Id::new("picker_shot")).fixed_pos(egui::pos2(430.0, 120.0)).show(ui.ctx(), |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            for (code, name) in names {
                                ui.label(format!("{code}: {name}"));
                            }
                        });
                    });
                }
            });
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            buffer.fill([20, 20, 20, 255]);
            let mut target = BufferMutRef::new(buffer, w, h);
            renderer.render(&mut target, &prims, &out.textures_delta, out.pixels_per_point);
        };
        let mut buffer = Vec::new();
        // Default window width and the minimum width (`min_inner_size` in `run`). Tall enough
        // that the whole Mods page fits without scrolling.
        let sizes = [("", 1000usize, 2000usize), ("-narrow", 600, 2400)];
        let languages = ["en", "ko", "ja", "zh", "zh-Hant", "de", "fr", "ru", "pl", "es-419", "en"];
        for code in languages {
            manager.i18n.set(code);
            for (suffix, w, h) in sizes {
                for (tab, name) in [(Tab::Mods, "mods"), (Tab::Settings, "settings")] {
                    manager.tab = tab;
                    for _ in 0..4 {
                        frame(&mut manager, false, w, h, &mut buffer);
                    }
                    write_png(&out_dir.join(format!("manager-{code}-{name}{suffix}.png")), w, h, &buffer);
                }
            }
            // The language list on its own shot, so it does not cover the Settings page.
            manager.tab = Tab::Settings;
            for _ in 0..4 {
                frame(&mut manager, true, 1000, 820, &mut buffer);
            }
            write_png(&out_dir.join(format!("manager-{code}-picker.png")), 1000, 820, &buffer);
            let has_extra = manager.fonts_extra;
            assert_eq!(has_extra, !fonts::candidates(code).is_empty() && !manager.fonts_warned.iter().any(|c| c == code), "{code}");
        }
        eprintln!("shots in {}", out_dir.display());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn split_mark_reads_report_marks() {
        assert_eq!(split_mark("+ Backup done"), (Some('+'), "Backup done"));
        assert_eq!(split_mark("x Failed: file"), (Some('x'), "Failed: file"));
        assert_eq!(split_mark("= kept"), (Some('='), "kept"));
        assert_eq!(split_mark("+nospace"), (None, "+nospace"));
        assert_eq!(split_mark("Installed 3"), (None, "Installed 3"));
        assert_eq!(split_mark("x"), (None, "x"));
    }

    #[test]
    fn window_icon_data_is_a_64_px_rgba_image() {
        assert_eq!(ICON_RGBA.len(), ICON_SIZE * ICON_SIZE * 4);
        // The badge is a hexagon: the corner pixel is transparent, the centre is opaque.
        assert_eq!(ICON_RGBA[3], 0);
        assert_eq!(ICON_RGBA[(ICON_SIZE / 2 * ICON_SIZE + ICON_SIZE / 2) * 4 + 3], 255);
    }

    #[test]
    fn every_new_card_string_exists_in_every_locale() {
        let keys = [
            "rs_search", "rs_filter_all", "rs_filter_live", "rs_filter_parked", "rs_filter_update", "rs_group_inventory",
            "rs_group_crafting", "rs_group_map", "rs_group_ui", "rs_group_other", "rs_no_match", "rs_help_tip",
        ];
        let mut i = I18n::new("en");
        for (code, _, _) in i18n::codes() {
            i.set(code);
            for key in keys {
                let text = i.t(key);
                assert!(text != key && !text.is_empty(), "{code}: {key}");
            }
        }
        // Translated, not copied from English (Map and Help are the same word in some languages).
        let en = I18n::new("en");
        for code in ["de", "fr", "ja", "ko", "ru", "zh"] {
            i.set(code);
            assert_ne!(i.t("rs_search"), en.t("rs_search"), "{code}");
        }
    }

    /// A Manager with the card-shot mod list (live, parked, missing, three outdated) on a fresh
    /// temp root, without the background NMS.py check and with the game-running check frozen.
    fn layout_manager(tag: &str) -> (Manager, PathBuf) {
        let root = temp_root(tag);
        write(&mods::mods_dir(&root).join("CraftFromStorage").join("craft_from_storage.json"), "{}\n");
        let mut manager = Manager::new(root.clone());
        manager.legacy_rx = None;
        manager.legacy = Some(Legacy::default());
        manager.running = false;
        manager.running_checked = Some(Instant::now() + std::time::Duration::from_secs(3600));
        manager.rows = mods::OUR_MODS
            .iter()
            .enumerate()
            .map(|(i, name)| ModRow {
                name: (*name).to_string(),
                state: if i % 5 == 3 { "parked" } else if i == 13 { "missing" } else { "live" }.to_string(),
                outdated: i == 2 || i == 4 || i == 8,
            })
            .collect();
        (manager, root)
    }

    /// One frame at `w` x `h` pixels (zoom as in the window), with optional input events.
    fn run_frame(ctx: &egui::Context, manager: &mut Manager, w: usize, h: usize, events: Vec<egui::Event>) -> egui::FullOutput {
        let ppp = ctx.zoom_factor();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / ppp, h as f32 / ppp))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| manager.draw(ui))
    }

    fn repaint_delay(out: &egui::FullOutput) -> std::time::Duration {
        out.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay)
    }

    fn rects(manager: &Manager, prefix: &str) -> Vec<(String, egui::Rect)> {
        manager.layout.iter().filter(|(n, _)| n.starts_with(prefix)).cloned().collect()
    }

    fn rect_of(manager: &Manager, name: &str) -> egui::Rect {
        manager.layout.iter().find(|(n, _)| n == name).map(|(_, r)| *r).unwrap_or_else(|| panic!("no {name} in {:?}", manager.layout))
    }

    /// Fonts and zoom take effect from the frame after they are set; then the layout must be final.
    fn warm_up(ctx: &egui::Context, manager: &mut Manager) {
        for _ in 0..3 {
            run_frame(ctx, manager, 1000, 900, Vec::new());
        }
    }

    /// No settling frame: the first frame at a width already has the final card rects, and the
    /// second frame at the same width draws them exactly the same.
    #[test]
    fn card_rects_are_final_on_the_first_frame() {
        let (mut manager, root) = layout_manager("rects_once");
        let ctx = egui::Context::default();
        warm_up(&ctx, &mut manager);
        for (w, h) in [(1000usize, 1500usize), (1400, 1200), (640, 2200)] {
            run_frame(&ctx, &mut manager, w, h, Vec::new());
            let first = rects(&manager, "card:");
            run_frame(&ctx, &mut manager, w, h, Vec::new());
            let second = rects(&manager, "card:");
            assert!(first.len() >= 10, "{first:?}");
            assert_eq!(first, second, "card rects moved between frame 1 and 2 at {w} px");
            // Cards of one row share their top and their height.
            for (_, a) in &first {
                for (_, b) in &first {
                    if a.top() == b.top() {
                        assert_eq!(a.height(), b.height(), "{a:?} {b:?}");
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Widening and narrowing the window step by step: at every width, a second frame at that
    /// width changes no rect of the window (cards, toolbar, footer, ...).
    #[test]
    fn resize_sweep_has_no_settling_frame() {
        let (mut manager, root) = layout_manager("rects_sweep");
        let ctx = egui::Context::default();
        warm_up(&ctx, &mut manager);
        let widths: Vec<usize> = (600..=2000).step_by(7).collect();
        for w in widths.iter().copied().chain(widths.iter().rev().copied()) {
            run_frame(&ctx, &mut manager, w, 1400, Vec::new());
            let first = manager.layout.clone();
            run_frame(&ctx, &mut manager, w, 1400, Vec::new());
            assert_eq!(first, manager.layout, "layout changed on the second frame at {w} px");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Idle, the window only wakes for the 2 s game-running check: no animation or layout keeps
    /// asking for frames (the software renderer would burn CPU).
    #[test]
    fn idle_window_does_not_repaint() {
        let (mut manager, root) = layout_manager("idle");
        let ctx = egui::Context::default();
        // A few frames for the start-up fades (fonts, zoom, the scroll bar appearing).
        let mut out = run_frame(&ctx, &mut manager, 1000, 900, Vec::new());
        for _ in 0..20 {
            out = run_frame(&ctx, &mut manager, 1000, 900, Vec::new());
        }
        assert!(repaint_delay(&out) >= std::time::Duration::from_secs(1), "{:?} {:?}", repaint_delay(&out), ctx.repaint_causes());
        // A tab change animates the underline, then goes quiet again.
        manager.tab = Tab::Settings;
        let out = run_frame(&ctx, &mut manager, 1000, 900, Vec::new());
        assert!(repaint_delay(&out) < std::time::Duration::from_millis(100));
        let mut quiet = false;
        for _ in 0..30 {
            let out = run_frame(&ctx, &mut manager, 1000, 900, Vec::new());
            if repaint_delay(&out) >= std::time::Duration::from_secs(1) {
                quiet = true;
                break;
            }
        }
        assert!(quiet, "the tab animation never stopped asking for frames");
        let _ = fs::remove_dir_all(&root);
    }

    /// Every block starts on the same left gutter and ends on the same right edge; the footer
    /// marker and text share one midline.
    #[test]
    fn blocks_share_the_gutters() {
        let (mut manager, root) = layout_manager("gutters");
        let ctx = egui::Context::default();
        warm_up(&ctx, &mut manager);
        manager.message = "installed QuickRecharge".into();
        manager.message_area = Area::Mods;
        for (w, wide) in [(1000usize, true), (1400, true), (2000, true), (600, false)] {
            run_frame(&ctx, &mut manager, w, 1400, Vec::new());
            run_frame(&ctx, &mut manager, w, 1400, Vec::new());
            let screen = rect_of(&manager, "chrome");
            let gutter = screen.left() + st::GUTTER;
            let right = screen.right() - st::GUTTER;
            let near = |a: f32, b: f32, what: &str| assert!((a - b).abs() < 0.8, "{w} px: {what}: {a} vs {b}");
            near(rect_of(&manager, "logo").left(), gutter, "logo");
            near(rect_of(&manager, "search").left(), gutter, "search");
            near(rect_of(&manager, "footer:marker").left(), gutter, "footer marker");
            near(rect_of(&manager, "play").right(), right, "play");
            near(rect_of(&manager, "help").right(), right, "help");
            let cards = rects(&manager, "card:");
            let card_left = cards.iter().map(|(_, r)| r.left()).fold(f32::MAX, f32::min);
            let card_right = cards.iter().map(|(_, r)| r.right()).fold(f32::MIN, f32::max);
            near(card_right, right, "cards right");
            let content_gutter = if wide { rect_of(&manager, "sidebar").right() + st::GUTTER } else { gutter };
            near(card_left, content_gutter, "cards left");
            near(rect_of(&manager, "heading:rs_group_inventory").left(), content_gutter, "group heading");
            if wide {
                near(rect_of(&manager, "button:0").left(), content_gutter, "first toolbar button");
                near(rect_of(&manager, "search").right(), rect_of(&manager, "sidebar").right() - st::GUTTER, "search right");
            }
            // Search box and buttons: one height, one row.
            let search = rect_of(&manager, "search");
            for (_, b) in rects(&manager, "button:").into_iter().chain([(String::new(), rect_of(&manager, "help"))]) {
                assert_eq!(b.height(), search.height(), "{w} px");
                if wide {
                    near(b.top(), search.top(), "toolbar row");
                }
            }
            let footer = rect_of(&manager, "footer");
            assert_eq!(footer.height(), st::FOOTER_H);
            near(rect_of(&manager, "body").bottom(), footer.top(), "body meets footer");
            let marker = rect_of(&manager, "footer:marker");
            let text = rect_of(&manager, "footer:text");
            let close = rect_of(&manager, "footer:close");
            near(marker.center().y, close.center().y, "marker and close midline");
            // The text's first line is centred on the same midline.
            assert!((text.top() + text.height() / 2.0 - marker.center().y).abs() < 1.0, "{w} px: {text:?} {marker:?}");
            near(close.right(), right, "footer close");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// Paints `out` into `buffer`. `pending` holds the texture changes (font atlas) of the frames
    /// that were run but not painted; the renderer needs all of them.
    fn render(
        ctx: &egui::Context,
        renderer: &mut egui_software_backend::EguiSoftwareRender,
        pending: &mut egui::TexturesDelta,
        out: egui::FullOutput,
        (w, h): (usize, usize),
        buffer: &mut Vec<[u8; 4]>,
    ) {
        use egui_software_backend::BufferMutRef;
        buffer.resize(w * h, [0u8; 4]);
        pending.append(out.textures_delta);
        let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
        buffer.fill([20, 20, 20, 255]);
        let mut target = BufferMutRef::new(buffer, w, h);
        renderer.render(&mut target, &prims, pending, out.pixels_per_point);
        *pending = egui::TexturesDelta::default();
    }

    /// Headless screenshots of the card-grid manager: en, ko and de at 600, 1000, 1400 and 2000
    /// px: plain, with the NMS.py panel, after a migration, with open cards, with the help
    /// popover, with the filter and search in use, a hovered card, the four footer states, a
    /// normal-height window and Settings. Each shot is taken once animations have finished. Also
    /// a resize sweep (1180 to 1260 px in 2 px steps, two frames each): the card rects must not
    /// change between the frames; every 8 px a slice of the grid goes into `sweep-en.png`.
    /// Run: `cargo test -p nms-launcher card_shots -- --ignored`; PNGs `cards-*.png` in
    /// `NMS_RS_SHOTS` (default `%TEMP%\nms-rs-shots`).
    #[test]
    #[ignore]
    fn card_shots() {
        use crate::migrate::{Line, Mark, Report};
        use egui_software_backend::{ColorFieldOrder, EguiSoftwareRender};
        let out_dir = std::env::var_os("NMS_RS_SHOTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("nms-rs-shots"));
        fs::create_dir_all(&out_dir).unwrap();
        let (mut manager, root) = layout_manager("cardshots");
        assert!(outdated_count(&manager.rows) >= 2);
        let legacy = Legacy {
            nms_py: Some(root.join("NMS.py")),
            nms_py_bytes: 80_068_745,
            nms_py_files: 2848,
            python_files: (0..16).map(|i| root.join(format!("x{i}.py"))).collect(),
            python_mods: ["BiggerItemGrids", "CraftFromStorage", "HideEmptyHover", "QuickRecharge"].iter().map(|s| (*s).to_string()).collect(),
            old_option: true,
            wrappers: vec![root.join("NMS.py Setup.cmd")],
            py_open: Vec::new(),
        };
        let line = |mark, key, detail: &str| Line { mark, key, detail: detail.into(), detail_key: None };
        let report = Report {
            lines: vec![
                line(Mark::Done, "mig_backup", r"NMS.rs\backup\migrate-20261014-101500 (31)"),
                line(Mark::Done, "mig_carried", "manager.json"),
                line(Mark::Done, "mig_updated", "updated QuickTransferAll, SmallerInventorySlots"),
                line(Mark::Kept, "mig_kept_edited", r"GAMEDATA\MODS\QuickTransferAll\QuickDeposit.lua"),
                line(Mark::Done, "mig_python", "16"),
                line(Mark::Fail, "mig_py_partial", r"NMS.py\runtime\python310.dll"),
                line(Mark::Warn, "mig_partly", ""),
                line(Mark::Done, "mig_undo", r"NMS.rs\backup\migrate-20261014-101500"),
            ],
            ok: false,
            backup: None,
        };
        let ctx = egui::Context::default();
        let mut renderer = EguiSoftwareRender::new(ColorFieldOrder::Rgba);
        let mut buffer = Vec::new();
        let mut pending = egui::TexturesDelta::default();
        // Frames until nothing animates any more (at most a second of frames), then the picture.
        // `hover`: after the first frame the pointer rests on the second card's footer row (no
        // tooltip there).
        let mut shot = |manager: &mut Manager, w: usize, h: usize, hover: bool, buffer: &mut Vec<[u8; 4]>, name: String| {
            let mut out = None;
            for i in 0..60 {
                let pointer = (hover && i > 0)
                    .then(|| rects(manager, "card:").get(1).map(|(_, r)| egui::pos2(r.left() + r.width() * 0.45, r.bottom() - 18.0)))
                    .flatten();
                // Without hover the pointer leaves the window, so no shot inherits the last one's.
                let events = pointer.map_or_else(|| vec![egui::Event::PointerGone], |p| vec![egui::Event::PointerMoved(p)]);
                let o = run_frame(&ctx, manager, w, h, events);
                let quiet = repaint_delay(&o) >= std::time::Duration::from_secs(1);
                if let Some(old) = out.replace(o) {
                    pending.append(old.textures_delta);
                }
                if i >= 3 && (quiet || manager.job.is_some()) {
                    break;
                }
            }
            render(&ctx, &mut renderer, &mut pending, out.unwrap(), (w, h), buffer);
            write_png(&out_dir.join(name), w, h, buffer);
        };
        let reset = |manager: &mut Manager| {
            manager.legacy = Some(Legacy::default());
            manager.message.clear();
            manager.message_area = Area::Mods;
            manager.failed = false;
            manager.help_open = false;
            manager.filter = Filter::All;
            manager.search.clear();
            manager.tab = Tab::Mods;
            manager.running = false;
            manager.job = None;
        };
        for code in ["en", "ko", "de"] {
            manager.i18n.set(code);
            for (w, h) in [(600usize, 2400usize), (1000, 1500), (1400, 1200), (2000, 1100)] {
                // (name, panel, report, help, filter, search, hover)
                type Variant = (&'static str, bool, bool, bool, Filter, &'static str, bool);
                let variants: [Variant; 6] = [
                    ("plain", false, false, false, Filter::All, "", false),
                    ("panel", true, false, false, Filter::All, "", false),
                    ("result", false, true, false, Filter::All, "", false),
                    ("help", false, false, true, Filter::All, "", false),
                    ("filter", false, false, false, Filter::Update, "quick", false),
                    ("hover", false, false, false, Filter::All, "", true),
                ];
                for (name, panel, result, help, filter, search, hover) in variants {
                    reset(&mut manager);
                    manager.legacy = Some(if panel { legacy.clone() } else { Legacy::default() });
                    if result {
                        manager.message = report.render(&manager.i18n);
                        manager.message_area = Area::Migrate;
                        manager.failed = true;
                    }
                    manager.help_open = help;
                    manager.filter = filter;
                    manager.search = search.to_string();
                    shot(&mut manager, w, h, hover, &mut buffer, format!("cards-{code}-{name}-{w}.png"));
                }
                // The footer status bar: an error, a success, a running job and the game running.
                let states: [(&str, bool, Area, &str); 4] = [
                    ("error", true, Area::Play, "Could not start the game: The request is not supported. (os error 50)"),
                    ("success", false, Area::Mods, "installed QuickRecharge, RefinerGuard"),
                    ("working", false, Area::Mods, ""),
                    ("running", false, Area::Mods, ""),
                ];
                for (name, failed, area, text) in states {
                    reset(&mut manager);
                    manager.message = text.to_string();
                    manager.message_area = area;
                    manager.failed = failed;
                    manager.running = name == "running";
                    if name == "working" {
                        let (tx, rx2) = mpsc::channel();
                        std::mem::forget(tx);
                        manager.job = Some(Job { area: Area::Mods, label: manager.i18n.t("install_all"), rx: rx2, started: Instant::now() });
                    }
                    shot(&mut manager, w, 820, false, &mut buffer, format!("cards-{code}-footer-{name}-{w}.png"));
                }
                // A normal window: the page scrolls, the chrome, sidebar and footer stay.
                reset(&mut manager);
                manager.legacy = Some(legacy.clone());
                shot(&mut manager, w, 820, false, &mut buffer, format!("cards-{code}-window-{w}.png"));
                // Settings with the old-setup section filled (tall, so nothing scrolls away).
                manager.tab = Tab::Settings;
                shot(&mut manager, w, if w < 1000 { 1500 } else { 1000 }, false, &mut buffer, format!("cards-{code}-settings-{w}.png"));
            }
        }
        // Resize sweep: two frames per width, no card rect may differ between them.
        reset(&mut manager);
        manager.i18n.set("en");
        let (sw, sh, slice_top, slice_h) = (1260usize, 900usize, 150usize, 300usize);
        let mut strip: Vec<[u8; 4]> = Vec::new();
        let mut slices = 0usize;
        for w in (1180..=1260).step_by(2) {
            pending.append(run_frame(&ctx, &mut manager, w, sh, Vec::new()).textures_delta);
            let first = rects(&manager, "card:");
            let out = run_frame(&ctx, &mut manager, w, sh, Vec::new());
            assert_eq!(first, rects(&manager, "card:"), "sweep: card rects moved on the second frame at {w} px");
            if (w - 1180) % 8 == 0 {
                render(&ctx, &mut renderer, &mut pending, out, (w, sh), &mut buffer);
                for y in slice_top..slice_top + slice_h {
                    for x in 0..sw {
                        strip.push(if x < w { buffer[y * w + x] } else { [0, 0, 0, 255] });
                    }
                }
                // A thin separator line between slices.
                strip.extend(std::iter::repeat_n([255u8, 0, 255, 255], sw));
                slices += 1;
            } else {
                pending.append(out.textures_delta);
            }
        }
        write_png(&out_dir.join("sweep-en.png"), sw, slices * (slice_h + 1), &strip);
        eprintln!("shots in {}", out_dir.display());
        let _ = fs::remove_dir_all(&root);
    }

    /// The "Old NMS.py setup found" panel (Mods tab), the Settings section, and a finished
    /// migration's report, in en, ko and de at the default and the minimum width.
    /// Run: `cargo test -p nms-launcher migrate_shots -- --ignored`; PNGs `migrate-*.png` in
    /// `NMS_RS_SHOTS` (default `%TEMP%\nms-rs-shots`).
    #[test]
    #[ignore]
    fn migrate_shots() {
        use crate::migrate::{Line, Mark, Report};
        use egui_software_backend::{BufferMutRef, ColorFieldOrder, EguiSoftwareRender};
        let out_dir = std::env::var_os("NMS_RS_SHOTS")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("nms-rs-shots"));
        fs::create_dir_all(&out_dir).unwrap();
        let root = temp_root("migshots");
        let mut manager = Manager::new(root.clone());
        manager.legacy_rx = None;
        let mods_with_py = ["BiggerItemGrids", "CraftFromStorage", "HideEmptyHover", "QuickRecharge", "QuickTransferAll",
            "SmallerInventorySlots", "SphereSizedMining", "TradeUITweaks"];
        // The real 2026-10-08 backup: 81 MB, 2848 files, 16 .py/.pyc in eight mods.
        let legacy = Legacy {
            nms_py: Some(root.join("NMS.py")),
            nms_py_bytes: 80_068_745,
            nms_py_files: 2848,
            python_files: (0..16).map(|i| root.join(format!("x{i}.py"))).collect(),
            python_mods: mods_with_py.iter().map(|s| (*s).to_string()).collect(),
            old_option: true,
            wrappers: vec![root.join("NMS.py Setup.cmd")],
            py_open: Vec::new(),
        };
        let line = |mark, key, detail: &str| Line { mark, key, detail: detail.into(), detail_key: None };
        let report = Report {
            lines: vec![
                line(Mark::Done, "mig_backup", r"NMS.rs\backup\migrate-20261014-101500 (31)"),
                line(Mark::Done, "mig_carried", "manager.json"),
                line(Mark::Done, "mig_updated", "updated QuickTransferAll, SmallerInventorySlots"),
                line(Mark::Kept, "mig_kept_edited", r"GAMEDATA\MODS\QuickTransferAll\QuickDeposit.lua"),
                line(Mark::Done, "mig_python", "16"),
                line(Mark::Done, "mig_steam", "Steam account 111: launch option removed."),
                line(Mark::Done, "mig_deleted", "NMS.py Setup.cmd"),
                line(Mark::Fail, "mig_py_partial", r"NMS.py\runtime\python310.dll"),
                line(Mark::Kept, "mig_kept", ""),
                line(Mark::Warn, "mig_partly", ""),
                line(Mark::Done, "mig_undo", r"NMS.rs\backup\migrate-20261014-101500"),
            ],
            ok: false,
            backup: None,
        };
        let ctx = egui::Context::default();
        let mut renderer = EguiSoftwareRender::new(ColorFieldOrder::Rgba);
        let mut frame = |manager: &mut Manager, w: usize, h: usize, buffer: &mut Vec<[u8; 4]>| {
            buffer.resize(w * h, [0u8; 4]);
            let ppp = ctx.zoom_factor();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / ppp, h as f32 / ppp))),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| manager.draw(ui));
            let prims = ctx.tessellate(out.shapes, out.pixels_per_point);
            buffer.fill([20, 20, 20, 255]);
            let mut target = BufferMutRef::new(buffer, w, h);
            renderer.render(&mut target, &prims, &out.textures_delta, out.pixels_per_point);
        };
        let mut buffer = Vec::new();
        for code in ["en", "ko", "de"] {
            manager.i18n.set(code);
            for (suffix, w, h) in [("", 1000usize, 1100usize), ("-narrow", 600, 1500)] {
                manager.legacy = Some(legacy.clone());
                manager.message.clear();
                for (tab, name) in [(Tab::Mods, "panel"), (Tab::Settings, "settings")] {
                    manager.tab = tab;
                    for _ in 0..4 {
                        frame(&mut manager, w, h, &mut buffer);
                    }
                    write_png(&out_dir.join(format!("migrate-{code}-{name}{suffix}.png")), w, h, &buffer);
                }
                // After a run: the report lines in the panel, nothing left to migrate.
                manager.legacy = Some(Legacy::default());
                manager.tab = Tab::Mods;
                manager.message = report.render(&manager.i18n);
                manager.message_area = Area::Migrate;
                manager.failed = true;
                for _ in 0..4 {
                    frame(&mut manager, w, h, &mut buffer);
                }
                write_png(&out_dir.join(format!("migrate-{code}-result{suffix}.png")), w, h, &buffer);
            }
        }
        eprintln!("shots in {}", out_dir.display());
        let _ = fs::remove_dir_all(&root);
    }
}

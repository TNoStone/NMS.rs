//! User files under `<game>\NMS.rs\`: build id, merge order, conflicts, pack-logs.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use nms_core::paths;
use nms_core::spawn::command;
use serde_json::{json, Value};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

pub const MADE_FOR: &str = "183877";
const COMBINED_NAME: &str = "z_Combined Mods";
const MARKER: &[u8] = b"svn.hellogames.dev";

pub fn exe_build_id_from_bytes(bytes: &[u8]) -> Option<String> {
    let index = bytes.windows(MARKER.len()).position(|w| w == MARKER)?;
    let start = index.saturating_sub(32);
    let window = &bytes[start..index];
    let mut digits = Vec::new();
    for &byte in window.iter().rev() {
        if (48..=57).contains(&byte) {
            digits.push(byte);
        } else if !digits.is_empty() {
            break;
        }
    }
    if digits.is_empty() {
        return None;
    }
    digits.reverse();
    String::from_utf8(digits).ok()
}

pub fn exe_build_id(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    exe_build_id_from_bytes(&bytes)
}

pub fn build_status_from_id(ver: Option<&str>) -> (&'static str, String) {
    let line = format!("NMS.exe build {}. Loader made for {MADE_FOR}.", ver.unwrap_or("unknown"));
    let Some(ver) = ver else {
        return ("warn", line);
    };
    match (ver.parse::<i64>(), MADE_FOR.parse::<i64>()) {
        (Ok(current), Ok(made)) => {
            if current > made {
                ("warn", line + " This is a newer build; Rust features turn off if their code moved.")
            } else {
                ("ok", line)
            }
        }
        _ if ver == MADE_FOR => ("ok", line),
        _ => ("warn", line + " This is a different build; Rust features turn off if their code moved."),
    }
}

pub fn game_build_status(root: &Path) -> (&'static str, String) {
    let ver = exe_build_id(&root.join("Binaries").join("NMS.exe"));
    build_status_from_id(ver.as_deref())
}

pub fn game_build_line(root: &Path) -> String {
    game_build_status(root).1
}

#[allow(dead_code)]
pub fn game_is_running() -> bool {
    #[cfg(windows)]
    {
        let out = command("tasklist").args(["/FI", "IMAGENAME eq NMS.exe", "/NH"]).output();
        match out {
            Ok(out) => String::from_utf8_lossy(&out.stdout).contains("NMS.exe"),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        command("pgrep").args(["-x", "NMS.exe"]).output().map(|o| o.status.success()).unwrap_or(false)
    }
}

pub fn user_lines(path: &Path) -> Vec<String> {
    if !path.is_file() {
        return Vec::new();
    }
    let raw = std::fs::read(path).unwrap_or_default();
    let cow = String::from_utf8_lossy(&raw);
    let text = cow.strip_prefix('\u{feff}').unwrap_or(&cow);
    let mut names = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.split_once('#').map(|(left, _)| left).unwrap_or(raw_line).trim();
        if !line.is_empty() && !names.iter().any(|n| n == line) {
            names.push(line.to_string());
        }
    }
    names
}

pub fn merge_order_names(root: &Path) -> Vec<String> {
    user_lines(&paths::loader_dir(root).join("merge_order.txt"))
}

fn mtime_ns(path: &Path) -> u64 {
    path.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn mod_folders(root: &Path) -> Vec<PathBuf> {
    let mods = root.join("GAMEDATA").join("MODS");
    let Ok(entries) = std::fs::read_dir(&mods) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    folders.sort();
    folders.retain(|p| p.file_name().map(|n| n != COMBINED_NAME).unwrap_or(true));
    folders
}

fn ui_stamp(root: &Path) -> Value {
    let mut rows = Vec::new();
    for folder in mod_folders(root) {
        let ui = folder.join("UI");
        if !ui.is_dir() {
            continue;
        }
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        rows.push(json!([name, mtime_ns(&ui)]));
    }
    Value::Array(rows)
}

fn owners_cache(root: &Path) -> PathBuf {
    paths::loader_dir(root).join("cache").join("mbin_owners.json")
}

fn walk_ui_mbins(folder: &Path, found: &mut BTreeMap<String, Vec<String>>) {
    let ui = folder.join("UI");
    if !ui.is_dir() {
        return;
    }
    let folder_name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut stack = vec![ui];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let lower = name.to_ascii_lowercase();
            if !lower.ends_with(".mbin") {
                continue;
            }
            if lower.ends_with(".merged") {
                continue;
            }
            let rel = path.strip_prefix(folder).unwrap_or(&path);
            let key = rel.to_string_lossy().replace('\\', "/").to_ascii_uppercase();
            found.entry(key).or_default().push(folder_name.clone());
        }
    }
}

fn owners_from_json(value: &Value) -> Option<BTreeMap<String, Vec<String>>> {
    let map = value.as_object()?;
    let mut found = BTreeMap::new();
    for (path, names) in map {
        let list = names.as_array()?.iter().filter_map(|n| n.as_str().map(|s| s.to_string())).collect();
        found.insert(path.clone(), list);
    }
    Some(found)
}

pub fn mbin_owners(root: &Path, force: bool) -> BTreeMap<String, Vec<String>> {
    let stamp = ui_stamp(root);
    let cache = owners_cache(root);
    if !force && cache.is_file() {
        if let Ok(text) = std::fs::read_to_string(&cache) {
            if let Ok(saved) = serde_json::from_str::<Value>(&text) {
                if saved.get("stamp") == Some(&stamp) {
                    if let Some(owners) = saved.get("owners").and_then(owners_from_json) {
                        return owners;
                    }
                }
            }
        }
    }
    let mut found = BTreeMap::new();
    for folder in mod_folders(root) {
        walk_ui_mbins(&folder, &mut found);
    }
    let body = json!({"stamp": stamp, "owners": found});
    if let Ok(text) = serde_json::to_string_pretty(&body) {
        if let Some(parent) = cache.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&cache, text + "\n");
    }
    found
}

pub fn write_conflict_report(
    root: &Path,
    mut say: Option<&mut dyn FnMut(&str)>,
    jobs: &BTreeMap<String, Vec<String>>,
    leftovers: bool,
) -> PathBuf {
    let owners = if leftovers { mbin_owners(root, false) } else { BTreeMap::new() };
    let merged: BTreeMap<String, Vec<String>> =
        jobs.iter().map(|(path, names)| (path.to_ascii_uppercase(), names.clone())).collect();
    let clashes: BTreeMap<String, Vec<String>> =
        owners.into_iter().filter(|(path, names)| names.len() >= 2 && !merged.contains_key(path)).collect();
    let dest = paths::loader_dir(root).join("conflicts.txt");
    let mut lines = vec!["NMS.rs conflict report".to_string(), game_build_line(root), String::new()];
    if !merged.is_empty() {
        lines.push("Merged (the loader wrote one file in z_Combined Mods):".into());
        for (path, names) in &merged {
            lines.push(format!("  {path}"));
            for name in names {
                lines.push(format!("    {name}"));
            }
        }
        lines.push(String::new());
    }
    if !leftovers {
        lines.push("Leftover UI files are checked on Setup or Open conflicts.txt.".into());
        lines.push(String::new());
    } else if !clashes.is_empty() {
        lines.push("Still clash (two finished MBINs, no script merge). The game keeps one copy:".into());
        for (path, names) in &clashes {
            lines.push(format!("  {path}"));
            for name in names {
                lines.push(format!("    {name}"));
            }
        }
        lines.push(String::new());
        if let Some(cb) = say.as_mut() {
            cb(&format!("{} UI file(s) still clash. See {}.", clashes.len(), dest.display()));
        }
    } else {
        lines.push("No file clashes.".into());
        lines.push(String::new());
        if let Some(cb) = say.as_mut() {
            cb("No leftover file clashes.");
        }
    }
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&dest, lines.join("\n"));
    dest
}

fn local_stamps() -> (String, String) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut t = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut t) };
        let file = format!("{:04}{:02}{:02}-{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
        let note = format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
        (file, note)
    }
    #[cfg(not(windows))]
    {
        let d = std::time::SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let secs = d.as_secs();
        (format!("{secs}"), format!("{secs}"))
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn add_zip_file(zip: &mut ZipWriter<File>, options: SimpleFileOptions, arc: &str, path: &Path, out: &Path) -> io::Result<()> {
    if !path.is_file() || same_file(path, out) {
        return Ok(());
    }
    zip.start_file(arc, options)?;
    let mut src = File::open(path)?;
    io::copy(&mut src, zip)?;
    Ok(())
}

pub fn pack_logs(root: &Path) -> Result<PathBuf, String> {
    let logs = paths::loader_dir(root).join("logs");
    std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let (file_stamp, packed) = local_stamps();
    let out = logs.join(format!("nms-rs-logs-{file_stamp}.zip"));
    let file = File::create(&out).map_err(|e| e.to_string())?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut note = format!("{}\nPacked {packed}\n", game_build_line(root));
    for line in crate::proxy::status_lines(root) {
        note.push_str(&line);
        note.push('\n');
    }
    zip.start_file("note.txt", options).map_err(|e| e.to_string())?;
    zip.write_all(note.as_bytes()).map_err(|e| e.to_string())?;

    let nms_rs = paths::loader_dir(root);
    let mods = root.join("GAMEDATA").join("MODS");
    let pairs: Vec<(String, PathBuf)> = vec![
        ("launcher.log".into(), logs.join("launcher.log")),
        ("core.log".into(), logs.join("core.log")),
        ("conflicts.txt".into(), nms_rs.join("conflicts.txt")),
        ("merge_order.txt".into(), nms_rs.join("merge_order.txt")),
        ("combined.txt".into(), mods.join(COMBINED_NAME).join("combined.txt")),
        ("nms.py/launcher.log".into(), root.join("NMS.py").join("launcher.log")),
        ("nms.py/conflicts.txt".into(), root.join("NMS.py").join("conflicts.txt")),
    ];
    for (arc, path) in pairs {
        add_zip_file(&mut zip, options, &arc, &path, &out).map_err(|e| e.to_string())?;
    }
    if mods.is_dir() {
        if let Ok(folders) = std::fs::read_dir(&mods) {
            let mut folders: Vec<_> = folders.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
            folders.sort();
            for folder in folders {
                let Ok(entries) = std::fs::read_dir(&folder) else {
                    continue;
                };
                let mut logs_here: Vec<_> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && p.extension().map(|e| e == "log").unwrap_or(false))
                    .collect();
                logs_here.sort();
                let folder_name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                for path in logs_here {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let arc = format!("mods/{folder_name}/{name}");
                    add_zip_file(&mut zip, options, &arc, &path, &out).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nms-launcher-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).expect("temp Binaries");
        dir
    }

    #[test]
    fn build_id_from_bytes() {
        let marker = b"svn.hellogames.dev";
        let mut right = b"xxxx181442".to_vec();
        right.extend_from_slice(marker);
        assert_eq!(exe_build_id_from_bytes(&right).as_deref(), Some("181442"));

        let mut gap = b"123x456".to_vec();
        gap.extend_from_slice(marker);
        assert_eq!(exe_build_id_from_bytes(&gap).as_deref(), Some("456"));

        assert_eq!(exe_build_id_from_bytes(b"no marker here"), None);
    }

    #[test]
    fn build_status_rules() {
        let (kind, line) = build_status_from_id(Some("100000"));
        assert_eq!(kind, "ok");
        assert_eq!(line, format!("NMS.exe build 100000. Loader made for {MADE_FOR}."));

        let (kind, line) = build_status_from_id(Some(MADE_FOR));
        assert_eq!(kind, "ok");
        assert_eq!(line, format!("NMS.exe build {MADE_FOR}. Loader made for {MADE_FOR}."));

        let (kind, line) = build_status_from_id(Some("999999"));
        assert_eq!(kind, "warn");
        assert!(line.contains("newer build"), "{line}");
        assert!(line.contains("Rust features"), "{line}");

        let (kind, line) = build_status_from_id(None);
        assert_eq!(kind, "warn");
        assert_eq!(line, format!("NMS.exe build unknown. Loader made for {MADE_FOR}."));
    }

    #[test]
    fn user_lines_bom_and_comments() {
        let root = temp_root("lines");
        let path = root.join("merge_order.txt");
        let mut body = vec![0xEF, 0xBB, 0xBF];
        body.extend_from_slice(b"Alpha\n# comment\n\nBeta # tail\nAlpha\n");
        std::fs::write(&path, body).unwrap();
        assert_eq!(user_lines(&path), vec!["Alpha".to_string(), "Beta".to_string()]);
        std::fs::create_dir_all(paths::loader_dir(&root)).unwrap();
        std::fs::copy(&path, paths::loader_dir(&root).join("merge_order.txt")).unwrap();
        assert_eq!(merge_order_names(&root), vec!["Alpha".to_string(), "Beta".to_string()]);
        let _ = game_is_running();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn conflict_report_jobs_and_clash() {
        let root = temp_root("conflicts");
        for folder in ["ModA", "ModB"] {
            let ui = root.join("GAMEDATA").join("MODS").join(folder).join("UI");
            std::fs::create_dir_all(&ui).unwrap();
            std::fs::write(ui.join("X.MBIN"), b"x").unwrap();
        }
        let mut jobs = BTreeMap::new();
        jobs.insert("MODELS/A.MBIN".into(), vec!["ModA".into(), "ModB".into()]);
        let path = write_conflict_report(&root, None, &jobs, true);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("NMS.rs conflict report"), "{text}");
        assert!(text.contains("Merged (the loader wrote one file in z_Combined Mods):"), "{text}");
        assert!(text.contains("  MODELS/A.MBIN"), "{text}");
        assert!(text.contains("Still clash (two finished MBINs, no script merge). The game keeps one copy:"), "{text}");
        assert!(text.contains("  UI/X.MBIN"), "{text}");
        assert!(text.contains("    ModA"), "{text}");
        assert!(text.contains("    ModB"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pack_logs_holds_note_and_mod_log_not_itself() {
        let root = temp_root("pack");
        let folder = root.join("GAMEDATA").join("MODS").join("SomeMod");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mod.log"), b"hello").unwrap();
        std::fs::write(root.join("Binaries").join("winmm.dll"), b"NMS.rs proxy marker").unwrap();
        let zip_path = pack_logs(&root).expect("pack");
        assert!(zip_path.file_name().unwrap().to_string_lossy().starts_with("nms-rs-logs-"));
        let file = File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().unwrap().into_owned())
            .collect();
        names.sort();
        assert!(names.contains(&"note.txt".to_string()), "{names:?}");
        assert!(names.contains(&"mods/SomeMod/mod.log".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.ends_with(".zip")), "{names:?}");
        let mut note = archive.by_name("note.txt").unwrap();
        let mut body = String::new();
        io::Read::read_to_string(&mut note, &mut body).unwrap();
        assert!(body.contains("NMS.exe build"), "{body}");
        assert!(body.contains("Packed "), "{body}");
        assert!(body.contains("NMS.rs loader file: Binaries\\winmm.dll"), "{body}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

//! `nms-rs <command>`
//!
//!   install [--force] [--as winmm|version|xinput9_1_0]
//!                       copy the loader DLL (next to this exe) into <game>\Binaries: by default
//!                       the name NMS.rs already has there, else xinput9_1_0.dll
//!   uninstall           remove our loader DLL under every name, never another mod's file
//!   disable | enable    create or remove <game>\NMS.rs\disabled (kill switch)
//!   status              list the loaded mods and their settings (game must be running)
//!   set <mod> <key> <true|false|text>
//!   play [<game command>...]  merge UI, start the game, stay open until it exits
//!   prepare             merge UI scripts (Proton runs this before the game)
//!   pack-logs           zip NMS.rs logs and open the zip in Explorer
//!   conflicts           write NMS.rs\\conflicts.txt (leftover UI MBIN clashes)
//!   merge --plan        list UI merge scripts and jobs, without writing
//!   setup [--yes] [--no-shortcut] [--force]
//!   steam-option status | set | remove [--yes]
//!   mods park|unpark|install|update <name> | park-all | install-all | update-all | cleanup-python | strip-launch | status
//!   sizes apply --columns N --rows auto|N --window F --slots P [--no-filters]
//!   migrate [--yes]       move an old NMS.py (version 3) setup over to NMS.rs (asks first)
//!   manager              open the out-of-game window (also the default with no args)
//!
//! The game folder is found by walking up from this exe, then from the current folder.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nms_core::{ipc, paths};
use serde_json::{json, Value};

mod cards;
mod files;
mod fonts;
mod i18n;
mod manager_settings;
mod merge;
mod migrate;
mod mods;
mod palette;
mod play;
mod proxy;
mod setup;
mod shortcut;
mod steam;
mod style;
mod sizes;
mod store;
mod tools;
mod ui;

fn game_root() -> Option<PathBuf> {
    let from_exe = std::env::current_exe().ok().and_then(|e| e.parent().and_then(paths::find_game_root));
    from_exe.or_else(|| std::env::current_dir().ok().and_then(|d| paths::find_game_root(&d)))
}

pub(crate) fn install(root: &Path, force: bool) -> Result<String, String> {
    install_as(root, force, None)
}

/// Copy `<name>` from next to this exe into `Binaries\`. Never over another mod's file of that name
/// unless forced. With `--as`, our files under the other names go, so only one loader stays.
fn install_as(root: &Path, force: bool, wanted: Option<&str>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let here = exe.parent().ok_or("no exe folder")?;
    let installed = proxy::installed(root);
    let name = match wanted {
        Some(w) => proxy::NAMES
            .iter()
            .copied()
            .find(|n| n.eq_ignore_ascii_case(w) || n.trim_end_matches(".dll").eq_ignore_ascii_case(w))
            .ok_or_else(|| format!("unknown loader name {w}; use one of {}", proxy::NAMES.join(", ")))?,
        None => match installed.as_slice() {
            [one] => *one,
            _ => proxy::MAIN,
        },
    };
    let src = here.join(name);
    let dst = proxy::path(root, name);
    if !src.is_file() {
        if wanted.is_none() && installed.contains(&name) {
            // A "DLL only" file from Nexus; the main zip has no copy of that name to update it from.
            return Ok(format!("{} is NMS.rs; left as it is (update it from the NMS.rs {name} file)", dst.display()));
        }
        return Err(format!("{name} not found next to this program"));
    }
    if dst.is_file() && !proxy::is_ours(&dst) && !force {
        // Snap Angles ships an xinput and a winmm version; NMS.rs also comes as winmm.dll and version.dll.
        return Err(format!(
            "{} belongs to another mod; not replacing it (use --force). Keep it and install the NMS.rs \
             winmm.dll or version.dll file from Nexus instead (not winmm.dll if Snap Angles' winmm.dll is \
             there).",
            dst.display()
        ));
    }
    std::fs::create_dir_all(paths::loader_dir(root).join("logs")).map_err(|e| e.to_string())?;
    std::fs::copy(&src, &dst).map_err(|e| format!("copy failed (is the game running?): {e}"))?;
    let mut lines = vec![format!("installed {}", dst.display())];
    if wanted.is_some() {
        for other in installed.iter().filter(|n| **n != name) {
            let path = proxy::path(root, other);
            std::fs::remove_file(&path).map_err(|e| format!("remove failed (is the game running?): {e}"))?;
            lines.push(format!("removed {} (NMS.rs, replaced by {name})", path.display()));
        }
    }
    lines.extend(proxy::twice_warning(&proxy::installed(root)));
    Ok(lines.join("\n"))
}

/// Remove our loader DLL under every name. Another mod's file of the same name stays.
fn uninstall(root: &Path) -> Result<String, String> {
    let mut lines = Vec::new();
    for name in proxy::NAMES {
        let path = proxy::path(root, name);
        if !path.is_file() {
            continue;
        }
        if !proxy::is_ours(&path) {
            lines.push(format!("{} is not ours; leaving it", path.display()));
            continue;
        }
        std::fs::remove_file(&path).map_err(|e| format!("remove failed (is the game running?): {e}"))?;
        lines.push(format!("removed {}", path.display()));
    }
    if !lines.iter().any(|l| l.starts_with("removed")) {
        lines.insert(0, "nothing to remove".into());
    }
    Ok(lines.join("\n"))
}

fn kill_switch(root: &Path, disable: bool) -> Result<String, String> {
    let flag = paths::loader_dir(root).join("disabled");
    if disable {
        std::fs::create_dir_all(flag.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&flag, b"").map_err(|e| e.to_string())?;
        Ok("disabled: the game stays vanilla from the next start".into())
    } else {
        let _ = std::fs::remove_file(&flag);
        Ok("enabled from the next start".into())
    }
}

fn status() -> Result<String, String> {
    let reply = ipc::request(&json!({"op": "list"}).to_string())?;
    let mut out = String::new();
    for m in reply["mods"].as_array().into_iter().flatten() {
        out.push_str(&format!("{}\n", m["name"].as_str().unwrap_or("?")));
        for s in m["settings"].as_array().into_iter().flatten() {
            out.push_str(&format!("  {:<12} {:<28} {}\n", s["key"].as_str().unwrap_or(""), s["label"].as_str().unwrap_or(""), s["value"]));
        }
    }
    Ok(out.trim_end().to_string())
}

fn set(mod_name: &str, key: &str, raw: &str) -> Result<String, String> {
    let value = match raw {
        "true" | "on" => Value::Bool(true),
        "false" | "off" => Value::Bool(false),
        text => Value::String(text.to_string()),
    };
    let reply = ipc::request(&json!({"op": "set", "mod": mod_name, "key": key, "value": value}).to_string())?;
    if reply["ok"] == json!(true) {
        Ok(reply["message"].as_str().unwrap_or("done").to_string())
    } else {
        Err(reply["error"].as_str().unwrap_or("failed").to_string())
    }
}

/// `install [--force] [--as <name>]`, flags in any order.
fn install_cli(root: &Path, flags: &[&str]) -> Result<String, String> {
    let mut force = false;
    let mut wanted = None;
    let mut rest = flags.iter();
    while let Some(flag) = rest.next() {
        match *flag {
            "--force" => force = true,
            "--as" => wanted = Some(*rest.next().ok_or("--as needs a name: winmm, version or xinput9_1_0")?),
            other => return Err(format!("unknown install option {other}")),
        }
    }
    install_as(root, force, wanted)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    let manager = words.is_empty() || words == ["manager"];
    let needs_root = manager
        || matches!(words.first(), Some(&"install" | &"uninstall" | &"disable" | &"enable" | &"play" | &"prepare" | &"pack-logs" | &"conflicts" | &"merge" | &"mods" | &"sizes" | &"migrate"));
    let root = if needs_root {
        match game_root() {
            Some(r) => Some(r),
            None => {
                eprintln!("game folder not found (no Binaries\\NMS.exe above this program or the current folder)");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    if manager {
        let Some(r) = root else {
            return ExitCode::from(2);
        };
        return ui::run(r);
    }
    let result = match (words.as_slice(), &root) {
        (["install", flags @ ..], Some(r)) => install_cli(r, flags),
        (["uninstall"], Some(r)) => uninstall(r),
        (["disable"], Some(r)) => kill_switch(r, true),
        (["enable"], Some(r)) => kill_switch(r, false),
        (["status"], _) => status(),
        (["set", m, k, v], _) => set(m, k, v),
        (["play", cmd @ ..], Some(r)) => return play::cli_play(r, cmd),
        (["prepare"], Some(r)) => return play::cli_prepare(r),
        (["pack-logs"], Some(r)) => files::pack_logs(r).map(|path| {
            let _ = nms_core::spawn::reveal(&path);
            path.display().to_string()
        }),
        (["conflicts"], Some(r)) => {
            let path = files::write_conflict_report(r, None, &std::collections::BTreeMap::new(), true);
            Ok(path.display().to_string())
        }
        (["merge", "--plan"], Some(r)) => {
            crate::merge::Merge::for_game(r).print_plan();
            return ExitCode::SUCCESS;
        }
        (["setup", rest @ ..], _) => return setup::cli(rest),
        (["steam-option", rest @ ..], _) => steam::cli(rest),
        (["mods", rest @ ..], Some(r)) => crate::mods::cli(r, rest),
        (["sizes", rest @ ..], Some(r)) => crate::sizes::apply::cli(r, rest),
        (["migrate", rest @ ..], Some(r)) => return migrate::cli(r, rest),
        _ => Err("usage: nms-rs [manager] | install [--force] [--as winmm|version|xinput9_1_0] | uninstall | disable | enable | status | set <mod> <key> <value> | play [<game command>...] | prepare | pack-logs | conflicts | merge --plan | setup [--yes] [--no-shortcut] [--force] | steam-option status | set | remove [--yes] | mods park|unpark|install|update <name> | park-all | install-all | update-all | cleanup-python | strip-launch | status | sizes apply --columns N --rows auto|N --window F --slots P [--no-filters] | migrate [--yes]".into()),
    };
    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(text) => {
            eprintln!("{text}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Our files go under every name; another mod's xinput9_1_0.dll stays.
    #[test]
    fn uninstall_removes_ours_under_any_name_only() {
        let root = std::env::temp_dir().join(format!("nms-rs-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Binaries")).unwrap();
        std::fs::write(proxy::path(&root, "xinput9_1_0.dll"), b"MZ snap angles").unwrap();
        std::fs::write(proxy::path(&root, "winmm.dll"), b"MZ NMS.rs proxy marker").unwrap();
        std::fs::write(proxy::path(&root, "version.dll"), b"MZ NMS.rs proxy marker").unwrap();
        let text = uninstall(&root).unwrap();
        assert!(text.contains("winmm.dll") && text.contains("version.dll"), "{text}");
        assert!(text.contains("xinput9_1_0.dll is not ours; leaving it"), "{text}");
        assert!(proxy::path(&root, "xinput9_1_0.dll").is_file());
        assert!(proxy::installed(&root).is_empty());
        assert!(uninstall(&root).unwrap().starts_with("nothing to remove"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_flags() {
        let root = std::env::temp_dir();
        assert!(install_cli(&root, &["--as"]).unwrap_err().contains("needs a name"));
        assert!(install_cli(&root, &["--bogus"]).unwrap_err().contains("unknown install option"));
        assert!(install_cli(&root, &["--as", "dinput8"]).unwrap_err().contains("unknown loader name"));
    }
}

//! `nms-rs play` / `prepare`: merge UI, start the game, stay open until it exits.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use nms_core::log::Log;
use nms_core::paths;

use crate::store::{self, Store};

const GAME_EXE: &str = "NMS.exe";
const GAME_ID: &str = "275850";

struct Session {
    log: Log,
    log_path: PathBuf,
    problems: Vec<String>,
    said: Vec<String>,
}

impl Session {
    fn open(root: &Path) -> Self {
        let log_path = paths::loader_dir(root).join("logs").join("launcher.log");
        let log = Log::open(&log_path, true);
        Self { log, log_path, problems: Vec::new(), said: Vec::new() }
    }

    fn say(&mut self, message: &str) {
        self.said.push(message.to_string());
        let _ = writeln!(io::stdout(), "{message}");
        let _ = io::stdout().flush();
        self.log.info(message);
    }

    fn problem(&mut self, message: &str) {
        self.problems.push(message.to_string());
        self.say(&format!("PROBLEM: {message}"));
    }

    fn prepare(&mut self, root: &Path) {
        self.say("NMS.rs loader: prepare");
        self.say(&crate::files::game_build_line(root));
        let loaders = crate::proxy::installed(root);
        if loaders.is_empty() {
            self.say("The Rust mods are not installed. Run nms-rs install while the game is closed.");
        } else {
            self.say(&format!("NMS.rs loader file: Binaries\\{}", loaders.join(", Binaries\\")));
        }
        if let Some(warning) = crate::proxy::twice_warning(&loaders) {
            self.problem(&warning);
        }
        if paths::loader_dir(root).join("disabled").exists() {
            self.say("NMS.rs is disabled (NMS.rs\\disabled). The game starts without mods.");
        }
        for line in crate::mods::auto_install(root) {
            self.say(&line);
        }
        self.merge_ui(root);
    }

    fn merge_ui(&mut self, root: &Path) {
        let merge = crate::merge::Merge::for_game(root);
        let mut lines = Vec::new();
        let mut problems = Vec::new();
        let _ = merge.merge_installed_mods(
            &mut |m| lines.push(m.to_string()),
            &mut |m| problems.push(m.to_string()),
        );
        for line in lines {
            self.say(&line);
        }
        for message in problems {
            self.problem(&message);
        }
    }

    fn play(&mut self, root: &Path, command: &[&str]) {
        if command.is_empty() {
            self.play_by_hand(root);
            return;
        }
        if let Some(first) = command.first() {
            if !first_is_nms_exe(first) {
                self.problem(&format!("The launch command does not start {GAME_EXE} ({first}). NMS.rs needs it to."));
            }
        }
        self.prepare(root);
        self.start_game(command, None);
    }

    fn play_by_hand(&mut self, root: &Path) {
        if store::kind(root) == Store::GamePass {
            self.problem("Xbox Game Pass is not supported.");
            return;
        }
        if store::uses_direct_exe(root) {
            let exe = root.join("Binaries").join(GAME_EXE);
            let cwd = root.join("Binaries");
            let exe_s = exe.to_string_lossy().into_owned();
            if !first_is_nms_exe(&exe_s) {
                self.problem(&format!("The launch command does not start {GAME_EXE} ({exe_s}). NMS.rs needs it to."));
            }
            self.prepare(root);
            self.start_game(&[&exe_s], Some(&cwd));
            return;
        }
        self.prepare(root);
        match Command::new("explorer.exe").arg(format!("steam://rungameid/{GAME_ID}")).spawn() {
            Ok(_) => {
                self.say("Asking Steam to start No Man's Sky.");
                std::thread::sleep(Duration::from_secs(3));
            }
            Err(err) => {
                self.problem(&format!("Could not start the game ({err})."));
            }
        }
    }

    fn start_game(&mut self, command: &[&str], cwd: Option<&Path>) {
        self.say("Starting No Man's Sky.");
        let Some(program) = command.first() else {
            self.problem("Could not start the game (empty command).");
            return;
        };
        let mut cmd = Command::new(program);
        cmd.args(&command[1..]);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                self.say(&format!("Game PID {}.", child.id()));
                self.say("The game is running. This window closes when the game does.");
                match child.wait() {
                    Ok(_) => self.say("The game has closed."),
                    Err(err) => self.problem(&format!("Could not wait for the game ({err}).")),
                }
            }
            Err(err) => {
                self.problem(&format!("Could not start the game ({err})."));
            }
        }
    }

    fn finish(self) -> ExitCode {
        if self.problems.is_empty() {
            ExitCode::SUCCESS
        } else {
            println!();
            println!("Something needs attention:");
            for message in &self.problems {
                println!("  - {message}");
            }
            println!("This is also saved in {}.", self.log_path.display());
            ExitCode::FAILURE
        }
    }
}

pub(crate) fn first_is_nms_exe(first: &str) -> bool {
    Path::new(first).file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case(GAME_EXE)).unwrap_or(false)
}

pub(crate) fn cli_prepare(root: &Path) -> ExitCode {
    let mut session = Session::open(root);
    session.prepare(root);
    session.finish()
}

pub(crate) fn cli_play(root: &Path, command: &[&str]) -> ExitCode {
    let mut session = Session::open(root);
    session.play(root, command);
    session.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_game(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nms-launcher-play-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).expect("temp Binaries");
        dir
    }

    #[test]
    fn nms_exe_name_check_is_case_insensitive() {
        assert!(first_is_nms_exe("NMS.exe"));
        assert!(first_is_nms_exe("nms.EXE"));
        assert!(first_is_nms_exe(r"C:\Steam\steamapps\common\game\Binaries\NMS.exe"));
        assert!(!first_is_nms_exe("steam.exe"));
        assert!(!first_is_nms_exe(r"C:\foo\NMS.exe.bak"));
        assert!(!first_is_nms_exe(""));
    }

    #[test]
    fn prepare_runs_rust_merge_without_python() {
        let root = temp_game("prepare");
        let mut session = Session::open(&root);
        session.prepare(&root);
        assert!(
            session.said.iter().any(|line| line == "Checking UI merge."),
            "{:?}",
            session.said
        );
        assert!(
            !session.said.iter().any(|line| line.contains("UI merge skipped")),
            "{:?}",
            session.said
        );
        assert!(session.problems.is_empty(), "{:?}", session.problems);
        let _ = std::fs::remove_dir_all(&root);
    }
}

//! File logs: one per mod (`<module>.log` next to its settings) and one for the core.
//!
//! Log state changes once, not every frame. Writes never fail the caller.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct State {
    file: Option<File>,
    /// The file was already opened once in this game session, so turning the log back on appends.
    opened: bool,
}

pub struct Log {
    path: Option<PathBuf>,
    state: Mutex<State>,
}

impl Log {
    /// A log at `path`, writing only while enabled. The first open in a session truncates the file.
    pub fn open(path: &Path, enabled: bool) -> Self {
        let log = Self { path: Some(path.to_path_buf()), state: Mutex::new(State { file: None, opened: false }) };
        log.set_enabled(enabled);
        log
    }

    pub fn disabled() -> Self {
        Self { path: None, state: Mutex::new(State { file: None, opened: false }) }
    }

    /// Start or stop writing, live.
    pub fn set_enabled(&self, enabled: bool) {
        let Some(path) = &self.path else { return };
        let Ok(mut s) = self.state.lock() else { return };
        if !enabled {
            s.file = None;
            return;
        }
        if s.file.is_some() {
            return;
        }
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Empty the file once, then always append: a line another handle adds (a second NMS.rs
        // DLL's note in core.log) then stays, instead of being overwritten at this handle's cursor.
        if !s.opened {
            let _ = OpenOptions::new().create(true).write(true).truncate(true).open(path);
        }
        s.file = OpenOptions::new().create(true).append(true).open(path).ok();
        s.opened |= s.file.is_some();
    }

    pub fn info(&self, message: &str) {
        if let Ok(mut s) = self.state.lock() {
            if let Some(f) = s.file.as_mut() {
                let _ = writeln!(f, "{} {}", timestamp(), message);
                let _ = f.flush();
            }
        }
    }
}

#[cfg(windows)]
pub fn timestamp() -> String {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}

#[cfg(not(windows))]
pub fn timestamp() -> String {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:03}", d.as_secs(), d.subsec_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_lines_when_enabled() {
        let path = std::env::temp_dir().join(format!("nms-core-log-{}.log", std::process::id()));
        let log = Log::open(&path, true);
        log.info("hello");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.trim_end().ends_with(" hello"), "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn toggles_live_and_appends_after_the_first_open() {
        let path = std::env::temp_dir().join(format!("nms-core-log-live-{}.log", std::process::id()));
        std::fs::write(&path, "old session\n").unwrap();
        let log = Log::open(&path, false);
        log.info("dropped");
        log.set_enabled(true);
        log.info("one");
        log.set_enabled(false);
        log.info("dropped");
        log.set_enabled(true);
        log.info("two");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().map(|l| l.rsplit(' ').next().unwrap()).collect();
        assert_eq!(lines, ["one", "two"], "{text}");
        let _ = std::fs::remove_file(&path);
    }

    /// A line appended through another handle (the second-DLL note) survives later writes.
    #[test]
    fn keeps_lines_from_another_handle() {
        let path = std::env::temp_dir().join(format!("nms-core-log-shared-{}.log", std::process::id()));
        let log = Log::open(&path, true);
        log.info("first");
        let mut other = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(other, "x note").unwrap();
        drop(other);
        log.info("second");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().map(|l| l.rsplit(' ').next().unwrap()).collect();
        assert_eq!(lines, ["first", "note", "second"], "{text}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disabled_log_writes_nothing() {
        Log::disabled().info("ignored");
    }
}

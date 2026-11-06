//! Start the core once the DLL is loaded: log, kill switch, UI merge, mods, settings pipe.

use std::io;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

use nms_core::log::Log;
use nms_core::process::ProcessImage;
use nms_core::{paths, Registry};

const PREPARE_LOG_PREFIX: &str = "prepare:";
/// 128 blocks of 1 MB: room for the trampolines (~30 hooks) and caves, reserved not committed.
const HOLD_BLOCK: usize = 0x10_0000;
const HOLD_BLOCKS: usize = 128;

pub fn run() {
    let exe = std::env::current_exe().unwrap_or_default();
    let is_game = exe.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("NMS.exe"));
    let Some(root) = paths::game_root() else { return };
    if !is_game {
        return;
    }
    let dir = paths::loader_dir(&root);
    let dll = crate::own_file_name();
    if !claim_process() {
        second_copy_note(&dir, &dll);
        return;
    }
    let log = Arc::new(Log::open(&dir.join("logs").join("core.log"), true));
    log.info(&format!("nms-rs {} loaded into {} as {dll}", nms_core::ipc::VERSION, exe.display()));
    if dir.join("disabled").exists() {
        log.info("NMS.rs\\disabled exists: staying vanilla");
        return;
    }
    // The image snapshot comes first (patterns are searched in it, before any hook changes code),
    // then the per-frame hook, so it lands as early as possible (`RESEARCH-lifecycle-and-context.md` 6.2).
    let image = match ProcessImage::current() {
        Ok(i) => i,
        Err(e) => {
            log.info(&format!("cannot read the game image, staying vanilla: {e}"));
            return;
        }
    };
    log.info(&format!("image base {:#x}", image.base()));
    // Before the UI merge and the hooks: under Wine the space around the image is taken about
    // 1.7 s after start, and hooks then fail with "Cannot allocate memory". Blocks are handed
    // back one at a time when a hook or cave finds no room (nms_core::mem::release_held_one).
    let held = nms_core::mem::hold_near(image.base(), HOLD_BLOCK, HOLD_BLOCKS);
    log.info(&format!("near-code memory held: {held} of {HOLD_BLOCKS} blocks of {} MB", HOLD_BLOCK >> 20));
    crate::loader_settings::apply_at_boot(&image, &dir, &log);
    start_lifecycle(&image, &dir, &log);
    merge_ui(&log, &dir, spawn_prepare);

    let mut registry = Registry::default();
    registry.add(Box::new(crate::loader_settings::LoaderSettings::new(&dir, image.clone(), log.clone())));
    // Dev-only: the self-test enables and disables a hook live, so it never ships.
    #[cfg(feature = "selftest")]
    registry.add(Box::new(crate::selftest::SelfTest::new(&dir, image.clone(), log.clone())));
    nms_mods::register(&mut registry, &nms_mods::Ctx { root, image: image.clone(), log: log.clone() });
    log.info(&format!("mods: {}", registry.names().join(", ")));

    let registry = Arc::new(registry);
    nms_mods::install_options_menu(&image, registry.clone(), log.clone());

    let pipe_log = log.clone();
    nms_core::ipc::serve(registry, move |m| pipe_log.info(m));
    log.info("settings pipe ready");
}

/// True for the first NMS.rs DLL to start in this process. Two can be installed at once (for
/// example xinput9_1_0.dll from the main zip and winmm.dll from a "DLL only" file); the second
/// must only forward. A named mutex per process id decides; its handle stays open for good.
fn claim_process() -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::{CreateMutexW, GetCurrentProcessId};
    // SAFETY: plain call with no inputs.
    let pid = unsafe { GetCurrentProcessId() };
    let name: Vec<u16> = format!("Local\\NMS.rs-loader-{pid}\0").encode_utf16().collect();
    // SAFETY: NUL-terminated name, default security, not initially owned.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        // Cannot tell; boot as a single install would.
        return true;
    }
    // SAFETY: read right after CreateMutexW on this thread.
    unsafe { GetLastError() != ERROR_ALREADY_EXISTS }
}

/// The one line a second NMS.rs DLL writes. It waits first: the booting copy empties core.log
/// when it opens it (and appends after that, so this line stays).
fn second_copy_note(loader: &Path, dll: &str) {
    use std::io::Write;
    std::thread::sleep(std::time::Duration::from_secs(5));
    let line = format!(
        "second NMS.rs loader file {dll}: another NMS.rs file started first, this one only \
         passes calls through. Keep one NMS.rs file in Binaries."
    );
    let path = loader.join("logs").join("core.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {line}", nms_core::log::timestamp());
    }
}

/// Resolve the game context and hook `cGcApplication::Update`. Any failure is logged and the
/// mods still load; `NMS.rs\lifecycle_off` keeps the context but skips the per-frame hook.
fn start_lifecycle(image: &ProcessImage, loader: &Path, log: &Arc<Log>) {
    let game = match nms_core::game::live::init(image) {
        Ok(g) => g,
        Err(e) => {
            log.info(&format!("game context off: {e}"));
            return;
        }
    };
    log.info(&game.layout().describe());
    if loader.join("lifecycle_off").exists() {
        log.info("NMS.rs\\lifecycle_off exists: no per-frame hook");
        return;
    }
    match nms_core::game::live::install_lifecycle(image, game, log.clone()) {
        Err(e) => log.info(&format!("lifecycle off: {e}")),
        // Dev-only: the automatic lifecycle check never ships.
        #[cfg(feature = "selftest")]
        Ok(()) => crate::lifecycle_test::start(log.clone()),
        #[cfg(not(feature = "selftest"))]
        Ok(()) => {}
    }
}

fn spawn_prepare(mut cmd: Command) -> io::Result<Output> {
    cmd.output()
}

/// `nms-rs.exe prepare` with cwd `<game>\NMS.rs`. Each path is its own `Command` field, not a shell line.
/// No console window (`nms_core::spawn`); its MBINCompiler and hgpaktool children share that
/// windowless console.
fn prepare_command(exe: &Path, cwd: &Path) -> Command {
    let mut cmd = nms_core::spawn::command(exe);
    cmd.arg("prepare");
    cmd.current_dir(cwd);
    cmd
}

/// First work on the boot thread after DllMain. Never panics. Failure still lets hooks register.
fn merge_ui(log: &Log, loader: &Path, run_child: impl FnOnce(Command) -> io::Result<Output>) {
    let exe = loader.join("nms-rs.exe");
    if !exe.is_file() {
        log.info("UI merge skipped: NMS.rs\\nms-rs.exe is not installed.");
        return;
    }
    let cmd = prepare_command(&exe, loader);
    match run_child(cmd) {
        Ok(out) => {
            log_prepare_stream(log, &out.stdout);
            log_prepare_stream(log, &out.stderr);
            if out.status.success() {
                log.info("UI merge finished.");
            } else {
                match out.status.code() {
                    // Exit 1 is any problem prepare listed above (two loader files, say), not only a failed merge.
                    Some(code) => log.info(&format!("prepare reported a problem (exit {code}), see the prepare lines above.")),
                    None => log.info("prepare ended without an exit status."),
                }
            }
        }
        Err(e) => {
            log.info(&format!("UI merge failed to start: {e}"));
        }
    }
}

fn log_prepare_stream(log: &Log, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for line in text.lines() {
        if !line.is_empty() {
            log.info(&format!("{PREPARE_LOG_PREFIX} {line}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    fn temp_loader(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nms-proxy-boot-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp loader");
        dir
    }

    fn open_log(loader: &Path) -> (Log, PathBuf) {
        let path = loader.join("core-test.log");
        (Log::open(&path, true), path)
    }

    fn fake_output(code: u32, stdout: &[u8], stderr: &[u8]) -> Output {
        Output { status: ExitStatus::from_raw(code), stdout: stdout.to_vec(), stderr: stderr.to_vec() }
    }

    /// The first claim in a process wins; every later one (a second NMS.rs DLL) is refused.
    #[test]
    fn only_the_first_claim_boots() {
        assert!(claim_process());
        assert!(!claim_process());
        assert!(!claim_process());
    }

    #[test]
    fn prepare_command_is_nms_rs_prepare_in_loader_dir() {
        let loader = Path::new(r"C:\Games\No Man's Sky\NMS.rs");
        let exe = loader.join("nms-rs.exe");
        let cmd = prepare_command(&exe, loader);
        assert_eq!(Path::new(cmd.get_program()), exe.as_path());
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["prepare"]);
        assert_eq!(cmd.get_current_dir(), Some(loader));
    }

    #[test]
    fn missing_exe_skips_without_calling_runner() {
        let loader = temp_loader("missing");
        let (log, log_path) = open_log(&loader);
        let mut ran = false;
        merge_ui(&log, &loader, |_| {
            ran = true;
            Ok(fake_output(0, b"", b""))
        });
        assert!(!ran, "missing nms-rs.exe must not spawn");
        let text = fs::read_to_string(&log_path).unwrap();
        assert!(
            text.contains("UI merge skipped: NMS.rs\\nms-rs.exe is not installed."),
            "{text}"
        );
        assert!(!text.contains("UI merge failed"), "{text}");
        let _ = fs::remove_dir_all(&loader);
    }

    #[test]
    fn child_exit_1_is_logged_and_run_continues() {
        let loader = temp_loader("exit1");
        fs::write(loader.join("nms-rs.exe"), b"").expect("dummy nms-rs.exe");
        let (log, log_path) = open_log(&loader);
        merge_ui(&log, &loader, |cmd| {
            assert_eq!(Path::new(cmd.get_program()).file_name().and_then(|n| n.to_str()), Some("nms-rs.exe"));
            let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            assert_eq!(args, ["prepare"]);
            assert_eq!(cmd.get_current_dir(), Some(loader.as_path()));
            Ok(fake_output(1, b"stdout line\n", b"stderr line\n"))
        });
        let text = fs::read_to_string(&log_path).unwrap();
        assert!(text.contains("prepare: stdout line"), "{text}");
        assert!(text.contains("prepare: stderr line"), "{text}");
        assert!(text.contains("prepare reported a problem (exit 1)"), "{text}");
        assert!(!text.contains("UI merge finished."), "{text}");
        let _ = fs::remove_dir_all(&loader);
    }

    #[test]
    fn spawn_error_is_logged_and_run_continues() {
        let loader = temp_loader("spawn");
        fs::write(loader.join("nms-rs.exe"), b"").expect("dummy nms-rs.exe");
        let (log, log_path) = open_log(&loader);
        merge_ui(&log, &loader, |_| Err(io::Error::new(io::ErrorKind::Other, "injected spawn failure")));
        let text = fs::read_to_string(&log_path).unwrap();
        assert!(text.contains("UI merge failed to start: injected spawn failure"), "{text}");
        let _ = fs::remove_dir_all(&loader);
    }

    #[test]
    fn zero_exit_logs_finished() {
        let loader = temp_loader("ok");
        fs::write(loader.join("nms-rs.exe"), b"").expect("dummy nms-rs.exe");
        let (log, log_path) = open_log(&loader);
        merge_ui(&log, &loader, |_| Ok(fake_output(0, b"Checking UI merge.\n", b"")));
        let text = fs::read_to_string(&log_path).unwrap();
        assert!(text.contains("prepare: Checking UI merge."), "{text}");
        assert!(text.contains("UI merge finished."), "{text}");
        let _ = fs::remove_dir_all(&loader);
    }
}

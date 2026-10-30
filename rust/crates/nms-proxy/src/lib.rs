//! `xinput9_1_0.dll` placed in `Binaries\`. NMS.exe imports it at startup, so the mods load with
//! no injector. The four XInput functions pass straight through to the real System32 DLL.
//!
//! Alternative names (Cargo features `as-winmm`, `as-version`) for players whose xinput9_1_0.dll
//! belongs to another mod: `winmm.dll` (imported by NMS.exe) and `version.dll` (imported by the
//! PlayFab, DLSS and Streamline DLLs NMS.exe loads at startup). Those forward every export of the
//! System32 DLL (`forward.rs`). Only the first NMS.rs file to start boots (`boot::claim_process`).
//!
//! Proton: Wine prefers its builtin DLL, so the launch option needs
//! `WINEDLLOVERRIDES="xinput9_1_0=n,b" %command%` (or `winmm=n,b`, `version=n,b`).
//! Kill switch: a file named `disabled` in `<game>\NMS.rs\` keeps the game vanilla.

#![cfg(windows)]

mod boot;
#[cfg(nms_proxy_forward)]
mod forward;
mod loader_settings;
#[cfg(feature = "selftest")]
mod lifecycle_test;
#[cfg(feature = "selftest")]
mod selftest;
#[cfg(not(nms_proxy_forward))]
mod xinput;

use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{BOOL, HMODULE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

#[cfg(not(nms_proxy_forward))]
pub use xinput::{XInputGetCapabilities, XInputGetDSoundAudioDeviceGuids, XInputGetState, XInputSetState};

/// `nms-rs install` looks for these bytes to tell our proxy from another mod's DLL of the same name.
#[used]
#[no_mangle]
pub static NMS_RS_PROXY_MARKER: [u8; 20] = *b"NMS.rs proxy marker\0";

/// This DLL's module handle, for its own file name in the log.
static MODULE: AtomicUsize = AtomicUsize::new(0);

#[no_mangle]
#[allow(non_snake_case)]
extern "system" fn DllMain(module: HMODULE, reason: u32, _reserved: *mut core::ffi::c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module as usize, Ordering::Relaxed);
        // SAFETY: valid module handle passed by the loader.
        unsafe { DisableThreadLibraryCalls(module) };
        // DllMain only starts the boot thread and returns. No wait, no CreateProcess, no
        // LoadLibrary under the loader lock. Merge and hooks run on that thread after this returns.
        let _ = std::thread::Builder::new().name("nms-rs boot".into()).spawn(boot::run);
    }
    TRUE
}

/// File name of this DLL as loaded (`xinput9_1_0.dll`, `winmm.dll`, ...).
fn own_file_name() -> String {
    use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
    let module = MODULE.load(Ordering::Relaxed);
    if module == 0 {
        return "?".into();
    }
    let mut buf = [0u16; 1024];
    // SAFETY: the length passed matches the buffer; the handle came from DllMain.
    let n = unsafe { GetModuleFileNameW(module as HMODULE, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    match path.rsplit(['\\', '/']).next() {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "?".into(),
    }
}

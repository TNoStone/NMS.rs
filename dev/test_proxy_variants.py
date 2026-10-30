"""The alternative proxy DLLs (winmm.dll, version.dll) export what System32 exports and pass calls through.

    py -3 -I dev\\test_proxy_variants.py [folder]

Reads the files `release.py rust-build` wrote to <target>\\dist\\variants. Each one is copied under its
real name into a temp folder and loaded into this Python by full path. Its boot thread starts and
returns at once (this is not NMS.exe), so nothing touches a game install. Then some exports are
called through our stubs and compared with the System32 DLL: the first call takes the slow path
(load the real DLL, look the export up), the second the resolved slot; VerFindFileW has 8 arguments,
so stack arguments are covered too.
"""
import ctypes
import shutil
import sys
import tempfile
from ctypes import wintypes as w
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import release  # noqa: E402

SYSTEM32 = release._system32("kernel32.dll").parent


def load(path: Path) -> ctypes.WinDLL:
    # LOAD_WITH_ALTERED_SEARCH_PATH: a full path, so Windows never hands back the System32 copy.
    return ctypes.WinDLL(str(path), winmode=0x8)


def check_winmm(ours: ctypes.WinDLL, real: ctypes.WinDLL) -> None:
    for _ in range(2):
        a, b = ours.timeGetTime(), real.timeGetTime()
        assert abs(int(b) - int(a)) < 1000, (a, b)
    assert ours.timeBeginPeriod(1) == 0 and ours.timeEndPeriod(1) == 0
    assert ours.waveOutGetNumDevs() == real.waveOutGetNumDevs()
    caps_ours, caps_real = ctypes.create_string_buffer(64), ctypes.create_string_buffer(64)
    assert ours.timeGetDevCaps(caps_ours, 8) == real.timeGetDevCaps(caps_real, 8) == 0
    assert caps_ours.raw[:8] == caps_real.raw[:8]


def check_version(ours: ctypes.WinDLL, real: ctypes.WinDLL, built: Path) -> None:
    for lib in (ours, real):
        lib.VerLanguageNameW.argtypes = [w.DWORD, w.LPWSTR, w.DWORD]
        lib.GetFileVersionInfoSizeW.argtypes = [w.LPCWSTR, ctypes.c_void_p]
        lib.GetFileVersionInfoW.argtypes = [w.LPCWSTR, w.DWORD, w.DWORD, ctypes.c_void_p]
        lib.VerQueryValueW.argtypes = [ctypes.c_void_p, w.LPCWSTR, ctypes.POINTER(ctypes.c_void_p), ctypes.POINTER(w.UINT)]
    names = []
    for lib in (ours, ours, real):
        buf = ctypes.create_unicode_buffer(128)
        assert lib.VerLanguageNameW(0x409, buf, 128) > 0
        names.append(buf.value)
    assert len(set(names)) == 1, names
    sizes = [lib.GetFileVersionInfoSizeW(str(built), None) for lib in (ours, real)]
    assert sizes[0] == sizes[1] > 0, sizes
    data = ctypes.create_string_buffer(sizes[0])
    assert ours.GetFileVersionInfoW(str(built), 0, sizes[0], data)
    value, length = ctypes.c_void_p(), w.UINT()
    assert ours.VerQueryValueW(data, r"\VarFileInfo\Translation", ctypes.byref(value), ctypes.byref(length))
    lang, codepage = ctypes.cast(value, ctypes.POINTER(w.WORD))[0:2]
    for key, want in (("ProductName", "NMS.rs"), ("OriginalFilename", "version.dll")):
        path = rf"\StringFileInfo\{lang:04x}{codepage:04x}\{key}"
        assert ours.VerQueryValueW(data, path, ctypes.byref(value), ctypes.byref(length)), path
        assert ctypes.wstring_at(value.value) == want, (key, ctypes.wstring_at(value.value))
    # 8 arguments: the last four go on the stack.
    results = []
    for lib in (ours, real):
        lib.VerFindFileW.argtypes = [w.DWORD, w.LPCWSTR, w.LPCWSTR, w.LPCWSTR, w.LPWSTR, ctypes.POINTER(w.UINT), w.LPWSTR, ctypes.POINTER(w.UINT)]
        cur, dest = ctypes.create_unicode_buffer(260), ctypes.create_unicode_buffer(260)
        cur_len, dest_len = w.UINT(260), w.UINT(260)
        code = lib.VerFindFileW(0, "kernel32.dll", None, str(SYSTEM32), cur, ctypes.byref(cur_len), dest, ctypes.byref(dest_len))
        results.append((code, cur.value.lower(), dest.value.lower(), cur_len.value, dest_len.value))
    assert results[0] == results[1], results


def main() -> int:
    # Optional: another folder that holds winmm.dll and version.dll builds.
    folder = Path(sys.argv[1]) if len(sys.argv) > 1 else release.PROXY_VARIANT_DIR
    variants = {dll: folder / dll for dll in release.PROXY_VARIANTS}
    missing = [str(p) for p in variants.values() if not p.is_file()]
    if missing:
        raise SystemExit(f"Missing {missing}. Build them first: py -3 release.py rust-build --version <v>")
    # The loaded DLLs cannot be deleted while this process runs.
    with tempfile.TemporaryDirectory(ignore_cleanup_errors=True) as tmp:
        for dll, built in variants.items():
            release.check_proxy_exports(built, dll)
            copy = Path(tmp) / dll
            shutil.copyfile(built, copy)
            ours, real = load(copy), load(release._system32(dll))
            assert ours._handle != real._handle, f"{dll}: Windows returned the System32 copy"
            assert ctypes.c_char.in_dll(ours, "NMS_RS_PROXY_MARKER").value == b"N"
            if dll == "winmm.dll":
                check_winmm(ours, real)
            else:
                check_version(ours, real, copy)
            print(f"ok: {dll} passes calls through")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

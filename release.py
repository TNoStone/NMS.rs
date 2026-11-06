"""Build the NMS.rs release zip.

    py -3 release.py rust-build --version 4.0.0       build the shipped exe and DLL (cargo --profile dist:
                                                      no debug info, no symbols, no build-machine paths)
    py -3 release.py rust --version 4.0.0 --out DIR   write NMS.rs-<version>.zip and its .sha256 into DIR,
                                                      plus NMS.rs-<version>-winmm.zip and -version.zip
                                                      (Nexus "DLL only" files, see PROXY_VARIANTS)
    py -3 release.py proxy-exports                    rewrite rust/crates/nms-proxy/exports/*.txt from
                                                      this PC's System32 (dev only)

Run it with any python.org Python 3.9 or newer, from anywhere: every path is relative to this file.
The binaries are read from <target>/dist, where <target> is CARGO_TARGET_DIR when it is set and
rust/target otherwise. Nothing here touches a game install.
"""

from __future__ import annotations

import hashlib
import json
import os
import struct
import subprocess
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
RUST = ROOT / "rust"
BUILD = ROOT / "build"
MODS_SRC = ROOT / "mods"
# Default-size game files of the four UI mods (modified MBINs). The manager's "Apply sizes" rebuilds
# BiggerItemGrids and SmallerInventorySlots to the player's pick on their own machine.
PRESETS = ROOT / "presets"
DEFAULT_PRESETS = {
    "BiggerItemGrids": PRESETS / "BiggerItemGrids" / "8x3-window1.4",
    "SmallerInventorySlots": PRESETS / "SmallerInventorySlots" / "70pct",
    "CompactTooltips": PRESETS / "CompactTooltips" / "default",
    "CompactQuickTransfer": PRESETS / "CompactQuickTransfer" / "default",
}


def _target_dir() -> Path:
    env = os.environ.get("CARGO_TARGET_DIR")
    return Path(env).resolve() if env else RUST / "target"


RUST_TARGET = _target_dir() / "dist"

# The loader DLL under other names NMS.exe loads from Binaries at startup, for players whose
# xinput9_1_0.dll belongs to another mod (Snap Angles, mod-warning skippers). {file name: Cargo
# feature}. Each goes to Nexus as a "DLL only" zip; the main zip keeps Binaries/xinput9_1_0.dll.
PROXY_DLL = "xinput9_1_0.dll"
PROXY_VARIANTS = {"winmm.dll": "as-winmm", "version.dll": "as-version"}
# rust-build copies each variant here (cargo writes every build to <target>/dist/xinput9_1_0.dll).
PROXY_VARIANT_DIR = RUST_TARGET / "variants"
PROXY_EXPORTS = RUST / "crates" / "nms-proxy" / "exports"
# Our own exports on top of the System32 ones (DllMain, the install marker, nms-core's detour entry).
PROXY_OWN_EXPORTS = {"DllMain", "NMS_RS_PROXY_MARKER", "nms_rs_detour_call_v1", "nms_rs_detour_entry_v1"}

# Every folder the manager installs from (NMS.rs/stock/<Mod>). Mods with only Rust code still get a
# stock.json entry so the manager knows their version. NMSrsMenu is lua only (the pause-menu row of
# the in-game Mods page).
RUST_STOCK_MODS = (
    "BiggerItemGrids",
    "SmallerInventorySlots",
    "CompactTooltips",
    "CompactQuickTransfer",
    "CraftFromStorage",
    "QuickTransferAll",
    "SphereSizedMining",
    "TradeUITweaks",
    "QuickRecharge",
    "HideEmptyHover",
    "NMSrsMenu",
    "GalaxyMapFilters",
    "PickerTweaks",
    "RefinerGuard",
    "ToolModeBinds",
)
RUST_README = (
    "NMS.rs {version}\n\n"
    "Extract this zip into your No Man's Sky game folder (the one that holds Binaries\n"
    "and GAMEDATA), so that Binaries\\xinput9_1_0.dll and NMS.rs\\nms-rs.exe land there.\n\n"
    "1. Run NMS.rs\\nms-rs.exe once to open the manager. Install the mods you want,\n"
    "   park the ones you do not, and pick your grid and slot sizes.\n"
    "2. Press Play in Steam. No launch option is needed.\n\n"
    "If you used version 3 (NMS.py), the manager finds the old setup and asks first.\n"
    "Migrate now backs up your settings to NMS.rs\\backup, then removes the old Python\n"
    "files, the old Steam launch option and the NMS.py folder. Your mod settings stay.\n\n"
    "Mod folders in NMS.rs\\stock are the copies the manager installs from.\n"
    "To remove everything, delete Binaries\\xinput9_1_0.dll and the NMS.rs folder.\n"
    "\nLinux / Proton (beta): the manager shows the one Steam launch option you need,\n"
    "WINEDLLOVERRIDES=\"xinput9_1_0=n,b\" %command%\n"
    "Without it the game starts but the mods do not load. If anything fails, use Pack logs in\n"
    "the manager's Settings tab and send the zip.\n"
    "\nIf another mod already uses Binaries\\xinput9_1_0.dll, keep that file and get the NMS.rs\n"
    "\"DLL only\" file named winmm or version from the Nexus page instead (see its README).\n"
)
# README of the "DLL only" zips. {dll}: winmm.dll; {stem}: winmm.
VARIANT_README = (
    "NMS.rs {version}: loader DLL as Binaries\\{dll}\n\n"
    "This file is only for players whose Binaries\\xinput9_1_0.dll belongs to another mod. It is\n"
    "the same NMS.rs loader as the main file, under a different name that No Man's Sky also loads.\n\n"
    "1. Install the main NMS.rs {version} zip first, but keep the other mod's\n"
    "   Binaries\\xinput9_1_0.dll (do not let the main zip replace it).\n"
    "2. Extract this zip into the game folder, so that Binaries\\{dll} lands next to NMS.exe.\n"
    "3. Keep only one NMS.rs loader file in Binaries. If two are there, only the first one runs\n"
    "   the mods and the manager shows a warning.\n"
    "4. When you update NMS.rs, get this file again in the same version as the main zip.\n\n"
    "{dll} can belong to only one mod. If another mod already ships a {dll} (Snap Angles has a\n"
    "winmm.dll version), use the other NMS.rs file instead.\n\n"
    "To remove it, delete Binaries\\{dll}.\n"
    "\nLinux / Proton (beta): the launch option for this file is\n"
    "WINEDLLOVERRIDES=\"{stem}=n,b\" %command%\n"
)


def _user_file(path: Path) -> bool:
    """Settings and logs a mod writes for itself; size presets (*_preset.json) are shipped."""
    name = path.name.lower()
    return (
        name.endswith(".log")
        or (name.endswith(".json") and not name.endswith("_preset.json"))
        or name in ("disabled.txt", "merge_order.txt", "conflicts.txt")
    )


def _rust_keep(path: Path) -> bool:
    """Shipped stock file: no caches, logs, user json, Python files or secrets."""
    if "__pycache__" in path.parts or path.suffix.lower() in (".py", ".pyc", ".log"):
        return False
    if path.name == ".env":
        return False
    return not _user_file(path)


def _sources(folder: Path) -> dict[str, Path]:
    if not folder.is_dir():
        return {}
    return {p.relative_to(folder).as_posix(): p for p in sorted(folder.rglob("*")) if p.is_file()}


def rust_files(version: str) -> dict[str, Path]:
    """{path inside the zip: source}. Paths are relative to the game folder."""
    dll, exe = RUST_TARGET / "xinput9_1_0.dll", RUST_TARGET / "nms-rs.exe"
    for path in (dll, exe):
        if not path.is_file():
            raise SystemExit(f"Missing {path}. Build it first: py -3 release.py rust-build --version {version}")
    files: dict[str, Path] = {"Binaries/xinput9_1_0.dll": dll, "NMS.rs/nms-rs.exe": exe}
    versions: dict[str, str] = {}
    for name in RUST_STOCK_MODS:
        sources = _sources(MODS_SRC / name)
        preset = DEFAULT_PRESETS.get(name)
        if preset is not None:
            if not (preset / "UI").is_dir():
                raise SystemExit(f"Missing the default preset {preset}")
            sources.update(_sources(preset))
        for relative, source in sources.items():
            # Per-mod README.txt is for this repo; players read the manager and README.txt.
            if relative == "README.txt" or not _rust_keep(source):
                continue
            files[f"NMS.rs/stock/{name}/{relative}"] = source
        versions[name] = version
    BUILD.mkdir(parents=True, exist_ok=True)
    info = BUILD / "stock_rust.json"
    info.write_text(json.dumps(versions, indent=2) + "\n", encoding="utf-8")
    files["NMS.rs/stock/stock.json"] = info
    # No default settings files: every Rust mod writes its own defaults on first load.
    readme = BUILD / "README-rust.txt"
    readme.write_text(RUST_README.format(version=version), encoding="utf-8")
    files["README.txt"] = readme
    sums = BUILD / "SHA256SUMS-rust.txt"
    sums.write_text(
        "".join(f"{_sha256(source)} *{name}\n" for name, source in sorted(files.items())),
        encoding="utf-8",
    )
    files["NMS.rs/SHA256SUMS.txt"] = sums
    return files


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _zip(path: Path, files: dict[str, Path]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, source in sorted(files.items()):
            archive.write(source, name)
    print(f"Zipped {path.name} ({path.stat().st_size / 1e6:.1f} MB)")


def _write_sha256(path: Path) -> None:
    # The hash to post with the release; anyone can compare it with their download.
    path.with_name(f"{path.name}.sha256").write_text(f"{_sha256(path)} *{path.name}\n", encoding="utf-8")


def make_rust_zip(version: str, out: Path) -> Path:
    path = out / f"NMS.rs-{version}.zip"
    files = rust_files(version)
    # The dist folder holds whichever proxy cargo built last; it must be the xinput one.
    check_proxy_exports(files[f"Binaries/{PROXY_DLL}"], PROXY_DLL)
    _zip(path, files)
    _write_sha256(path)
    return path


def make_variant_zips(version: str, out: Path) -> list[Path]:
    """NMS.rs-<version>-<stem>.zip per PROXY_VARIANTS: Binaries/<dll> and a README."""
    made = []
    BUILD.mkdir(parents=True, exist_ok=True)
    for dll in PROXY_VARIANTS:
        stem = dll.removesuffix(".dll")
        source = PROXY_VARIANT_DIR / dll
        if not source.is_file():
            raise SystemExit(f"Missing {source}. Build it first: py -3 release.py rust-build --version {version}")
        check_proxy_exports(source, dll)
        readme = BUILD / f"README-{stem}.txt"
        readme.write_text(VARIANT_README.format(version=version, dll=dll, stem=stem), encoding="utf-8")
        path = out / f"NMS.rs-{version}-{stem}.zip"
        _zip(path, {f"Binaries/{dll}": source, "README.txt": readme})
        _write_sha256(path)
        made.append(path)
    return made


def pe_exports(path: Path) -> list[tuple[int, str | None]]:
    """(ordinal, name or None) for every export of a PE file, in ordinal order. Standard library only."""
    data = path.read_bytes()
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise SystemExit(f"{path} is not a PE file")
    sections, = struct.unpack_from("<H", data, pe + 6)
    opt_size, = struct.unpack_from("<H", data, pe + 20)
    opt = pe + 24
    magic, = struct.unpack_from("<H", data, opt)
    export_rva, = struct.unpack_from("<I", data, opt + (112 if magic == 0x20B else 96))
    table = [struct.unpack_from("<IIII", data, opt + opt_size + 40 * i + 8) for i in range(sections)]

    def off(rva: int) -> int:
        for vsize, va, rsize, raw in table:
            if va <= rva < va + max(vsize, rsize):
                return rva - va + raw
        raise SystemExit(f"{path}: RVA {rva:#x} is outside every section")

    if not export_rva:
        return []
    base, functions, names, at_functions, at_names, at_ordinals = struct.unpack_from("<IIIIII", data, off(export_rva) + 16)
    named: dict[int, list[str]] = {}
    for i in range(names):
        start = off(struct.unpack_from("<I", data, off(at_names) + 4 * i)[0])
        index, = struct.unpack_from("<H", data, off(at_ordinals) + 2 * i)
        named.setdefault(index, []).append(data[start:data.index(b"\0", start)].decode("ascii"))
    out: list[tuple[int, str | None]] = []
    for i in range(functions):
        if struct.unpack_from("<I", data, off(at_functions) + 4 * i)[0]:
            out += [(base + i, name) for name in named.get(i, [None])]
    return out


def _system32(dll: str) -> Path:
    return Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / dll


def read_export_list(dll: str) -> list[tuple[int, str | None]]:
    """exports/<stem>.txt: what the variant must export (the System32 table it was made from)."""
    lines = (PROXY_EXPORTS / f"{dll.removesuffix('.dll')}.txt").read_text(encoding="utf-8").splitlines()
    rows = [line.split(" ", 1) for line in lines if line.strip() and not line.startswith("#")]
    return [(int(ordinal), None if name == "-" else name) for ordinal, name in rows]


def write_export_lists() -> None:
    """Dev only: the export tables of this PC's System32 DLLs, which build.rs turns into stubs."""
    PROXY_EXPORTS.mkdir(parents=True, exist_ok=True)
    for dll in PROXY_VARIANTS:
        rows = pe_exports(_system32(dll))
        head = (f"# Exports of System32\\{dll}: ordinal and name, \"-\" when exported by ordinal only.\n"
                "# The NMS.rs proxy of that name forwards every one. Rewrite: py -3 release.py proxy-exports\n")
        body = "".join(f"{ordinal} {name or '-'}\n" for ordinal, name in rows)
        (PROXY_EXPORTS / f"{dll.removesuffix('.dll')}.txt").write_text(head + body, encoding="utf-8")
        print(f"{dll}: {len(rows)} exports")


def check_proxy_exports(built: Path, dll: str) -> None:
    """A proxy exports every export of the System32 DLL of its name (same name, and same ordinal for
    the listed ones) and nothing else but our own: every module that imports the name binds to it."""
    got = pe_exports(built)
    names = {name for _, name in got if name}
    if dll == PROXY_DLL:
        want = {name for _, name in pe_exports(_system32(dll)) if name}
        missing = sorted(want - names)
        extra = names - want - PROXY_OWN_EXPORTS
    else:
        listed = read_export_list(dll)
        have = set(got)
        missing = [f"{name or '-'}@{ordinal}" for ordinal, name in listed if (ordinal, name) not in have]
        extra = names - {name for _, name in listed if name} - PROXY_OWN_EXPORTS
        extra |= {f"#{o}" for o, n in got if n is None and (o, None) not in set(listed)}
        system = _system32(dll)
        if system.is_file():
            # A newer Windows may add exports: the list must still cover this PC's System32 DLL.
            missing += [f"{name}@{ordinal} (new in System32, run proxy-exports)"
                        for ordinal, name in pe_exports(system) if name and name not in names]
    if missing or extra:
        raise SystemExit(f"{built.name} as {dll}: missing exports {missing[:20]}, unexpected exports {sorted(extra)[:20]}")
    print(f"exports ok: {dll} ({len(got)})")


def rust_build(version: str) -> None:
    """Build the files the zip ships, without debug info or this machine's folder names in them."""
    sysroot = subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True, check=True).stdout.strip()
    cargo_home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    flags = [
        f"--remap-path-prefix={RUST}=src",
        f"--remap-path-prefix={_target_dir()}=target",
        f"--remap-path-prefix={cargo_home}=cargo",
        f"--remap-path-prefix={sysroot}=rustc",
        "-C", "link-arg=/PDBALTPATH:%_PDB%",
    ]
    # CARGO_ENCODED_RUSTFLAGS, not RUSTFLAGS: folder names may have spaces in them.
    env = dict(os.environ, CARGO_ENCODED_RUSTFLAGS="\x1f".join(flags), NMS_RS_VERSION=version)
    env.pop("RUSTFLAGS", None)
    # Each variant is the same crate with one feature, written to the same file name: build and
    # copy them out first, so the plain xinput9_1_0.dll build comes last and stays in dist.
    PROXY_VARIANT_DIR.mkdir(parents=True, exist_ok=True)
    for dll, feature in PROXY_VARIANTS.items():
        subprocess.run(
            ["cargo", "build", "--profile", "dist", "-p", "nms-proxy", "--features", feature],
            cwd=RUST, env=env, check=True,
        )
        (PROXY_VARIANT_DIR / dll).write_bytes((RUST_TARGET / PROXY_DLL).read_bytes())
        check_proxy_exports(PROXY_VARIANT_DIR / dll, dll)
    subprocess.run(
        ["cargo", "build", "--profile", "dist", "-p", "nms-launcher", "-p", "nms-proxy"],
        cwd=RUST, env=env, check=True,
    )
    check_proxy_exports(RUST_TARGET / PROXY_DLL, PROXY_DLL)
    leaks = rust_path_leaks()
    if leaks:
        raise SystemExit("Build paths still in the shipped files: " + ", ".join(leaks))
    for path in _shipped_binaries():
        print(f"{_sha256(path)} *{path.name}")


def _shipped_binaries() -> list[Path]:
    return [RUST_TARGET / "nms-rs.exe", RUST_TARGET / PROXY_DLL] + [PROXY_VARIANT_DIR / dll for dll in PROXY_VARIANTS]


def rust_path_leaks() -> list[str]:
    """Names of this machine (user folder, repo folder) found inside the shipped exe and DLLs."""
    needles = {str(ROOT).lower(), str(Path.home()).lower(), Path.home().name.lower()}
    found = []
    for path in _shipped_binaries():
        data = path.read_bytes().lower()
        for needle in sorted(needles):
            if needle and (needle.encode() in data or needle.encode("utf-16-le") in data):
                found.append(f"{path.name}: {needle}")
    return found


def main() -> int:
    args = sys.argv[1:]
    if args and args[0] == "rust-build":
        opts = args[1:]
        if len(opts) != 2 or opts[0] != "--version":
            raise SystemExit(__doc__)
        rust_build(opts[1])
        return 0
    if args and args[0] == "rust":
        opts = args[1:]
        if len(opts) != 4 or opts[0] != "--version" or opts[2] != "--out":
            raise SystemExit(__doc__)
        out = Path(opts[3]).resolve()
        make_rust_zip(opts[1], out)
        make_variant_zips(opts[1], out)
        return 0
    if args == ["proxy-exports"]:
        write_export_lists()
        return 0
    raise SystemExit(__doc__)


if __name__ == "__main__":
    raise SystemExit(main())

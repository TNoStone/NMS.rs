"""release.py rust: the zip has the DLL and exe, and nothing it should not ship.

    py -3 -I dev\test_release_rust.py
"""
import hashlib
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
import release  # noqa: E402

# The shipped files come from `release.py rust-build` (<target>\\dist). Without them, test with the dev
# build (cargo build --release -p nms-launcher -p nms-proxy). <target> follows CARGO_TARGET_DIR.
if not (release.RUST_TARGET / "nms-rs.exe").is_file():
    release.RUST_TARGET = release._target_dir() / "release"
    release.PROXY_VARIANT_DIR = release.RUST_TARGET / "variants"


def check_variant_zips() -> None:
    """NMS.rs-<v>-winmm.zip and -version.zip: the DLL under its own name, a README, nothing else."""
    if not all((release.PROXY_VARIANT_DIR / dll).is_file() for dll in release.PROXY_VARIANTS):
        if release.RUST_TARGET.name == "dist":
            raise SystemExit(f"no variant DLLs in {release.PROXY_VARIANT_DIR}: run release.py rust-build")
        print("variant zips skipped: dev build without variants")
        return
    with tempfile.TemporaryDirectory() as tmp:
        made = release.make_variant_zips("4.0.0", Path(tmp))
        assert [p.name for p in made] == ["NMS.rs-4.0.0-winmm.zip", "NMS.rs-4.0.0-version.zip"], made
        for path, dll in zip(made, release.PROXY_VARIANTS):
            sidecar = Path(tmp) / f"{path.name}.sha256"
            assert sidecar.read_text().split()[0] == release._sha256(path), f"bad {sidecar.name}"
            with zipfile.ZipFile(path) as z:
                assert sorted(z.namelist()) == [f"Binaries/{dll}", "README.txt"], z.namelist()
                readme = z.read("README.txt").decode()
                assert "—" not in readme, "em dash in README"
                assert f'WINEDLLOVERRIDES="{dll[:-4]}=n,b"' in readme, readme
                assert b"NMS.rs proxy marker" in z.read(f"Binaries/{dll}"), f"{dll} is not ours"
    print("ok: variant zips")


def main() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        path = release.make_rust_zip("4.0.0", Path(tmp))
        sidecar = Path(tmp) / f"{path.name}.sha256"
        assert sidecar.read_text().split()[0] == release._sha256(path), "bad .sha256 file"
        with zipfile.ZipFile(path) as z:
            names = z.namelist()
            listed = z.read("NMS.rs/SHA256SUMS.txt").decode().splitlines()
            exe_hash = hashlib.sha256(z.read("NMS.rs/nms-rs.exe")).hexdigest()
    assert any(line == f"{exe_hash} *NMS.rs/nms-rs.exe" for line in listed), "exe hash missing from SHA256SUMS"
    low = [n.lower() for n in names]
    assert "Binaries/xinput9_1_0.dll" in names, "no proxy DLL"
    assert "NMS.rs/nms-rs.exe" in names, "no nms-rs.exe"
    assert "README.txt" in names, "no README"
    py = [n for n in names if n.lower().endswith(".py")]
    assert not py, f"unexpected .py: {py}"
    assert not [n for n in names if "pulsetweaks" in n.lower()], "PulseTweaks shipped"
    assert not [n for n in low if n.endswith(".env") or n == ".env"], ".env shipped"
    assert not [n for n in low if "__pycache__" in n or n.endswith(".pyc")], "pycache shipped"
    assert not [n for n in low if n.endswith(".log")], "log shipped"
    assert not [n for n in names if n.startswith(("NMS.py/", "GAMEDATA/"))], "stray root"
    assert any(n.startswith("NMS.rs/stock/BiggerItemGrids/") for n in names), "no stock folder"
    # Custom glyphs the side-bar lua and the built MBIN point at; the game reads them from the mod folder.
    icons = "TEXTURES/UI/FRONTEND/ICONS/CATEGORIES/"
    sis = ("FILTERFOODS.DDS", "FILTERCUSTOM.DDS", "FILTEREGG.DDS", "FILTERBONE.DDS", "FILTERTREASURE.DDS")
    for mod, dds in ([("SmallerInventorySlots", n) for n in sis]
                     + [("QuickTransferAll", "QUICKDEPOSIT.DDS"), ("QuickTransferAll", "SUITLOCK.DDS")]):
        assert f"NMS.rs/stock/{mod}/{icons}{dds}" in names, f"{mod} icon {dds} not shipped"
    print(f"ok: {len(names)} files, no .py")
    check_variant_zips()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

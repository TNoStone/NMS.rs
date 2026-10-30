//! Our loader DLL in `Binaries\`. The main zip ships it as xinput9_1_0.dll; players whose
//! xinput9_1_0.dll belongs to another mod use the same loader built as winmm.dll or version.dll
//! (Nexus "DLL only" files). A file counts as ours by the marker bytes inside it, never by its name.

use std::path::{Path, PathBuf};

/// Every name NMS.rs ships under, the main one first.
pub const NAMES: [&str; 3] = ["xinput9_1_0.dll", "winmm.dll", "version.dll"];
pub const MAIN: &str = NAMES[0];
/// `NMS_RS_PROXY_MARKER` in nms-proxy, present in every variant.
const MARKER: &[u8] = b"NMS.rs proxy marker";

pub fn path(root: &Path, name: &str) -> PathBuf {
    root.join("Binaries").join(name)
}

pub fn is_ours(path: &Path) -> bool {
    std::fs::read(path).map(|b| b.windows(MARKER.len()).any(|w| w == MARKER)).unwrap_or(false)
}

/// The names under which an NMS.rs loader DLL sits in `Binaries\`, in `NAMES` order.
pub fn installed(root: &Path) -> Vec<&'static str> {
    NAMES.iter().copied().filter(|n| is_ours(&path(root, n))).collect()
}

/// Steam launch option for Proton, which otherwise prefers Wine's builtin DLL of that name.
pub fn wine_launch_option(name: &str) -> String {
    format!("WINEDLLOVERRIDES=\"{}=n,b\" %command%", name.trim_end_matches(".dll"))
}

/// "Two NMS.rs loader files ..." when more than one is installed; the first to load runs the mods.
pub fn twice_warning(names: &[&str]) -> Option<String> {
    (names.len() > 1).then(|| {
        format!(
            "NMS.rs loader files: {} are all in Binaries. Only one runs the mods; delete the ones you do not need while the game is closed.",
            names.join(", ")
        )
    })
}

/// Status lines for `prepare` and Pack logs.
pub fn status_lines(root: &Path) -> Vec<String> {
    let names = installed(root);
    let mut lines = vec![match names.as_slice() {
        [] => "NMS.rs loader file: none in Binaries".to_string(),
        many => format!("NMS.rs loader file: Binaries\\{}", many.join(", Binaries\\")),
    }];
    lines.extend(twice_warning(&names));
    let foreign: Vec<&str> = NAMES.iter().copied().filter(|n| path(root, n).is_file() && !names.contains(n)).collect();
    if !foreign.is_empty() {
        lines.push(format!("Other mods' files with an NMS.rs name: Binaries\\{}", foreign.join(", Binaries\\")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nms-proxy-names-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).unwrap();
        dir
    }

    #[test]
    fn ours_by_marker_not_by_name() {
        let root = temp_root("marker");
        std::fs::write(path(&root, "winmm.dll"), b"MZ...NMS.rs proxy marker\0...").unwrap();
        std::fs::write(path(&root, "xinput9_1_0.dll"), b"MZ snap angles").unwrap();
        assert_eq!(installed(&root), ["winmm.dll"]);
        assert_eq!(wine_launch_option("winmm.dll"), "WINEDLLOVERRIDES=\"winmm=n,b\" %command%");
        let lines = status_lines(&root);
        assert_eq!(lines[0], "NMS.rs loader file: Binaries\\winmm.dll");
        assert!(lines.iter().any(|l| l.contains("Other mods' files") && l.contains("xinput9_1_0.dll")), "{lines:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn two_of_ours_warn() {
        let root = temp_root("twice");
        for name in ["xinput9_1_0.dll", "version.dll"] {
            std::fs::write(path(&root, name), b"NMS.rs proxy marker").unwrap();
        }
        assert_eq!(installed(&root), ["xinput9_1_0.dll", "version.dll"]);
        assert!(twice_warning(&installed(&root)).unwrap().contains("xinput9_1_0.dll, version.dll"));
        assert!(twice_warning(&["winmm.dll"]).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

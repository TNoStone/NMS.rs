//! Text of the in-game Mods page in the 15 game languages.
//!
//! The strings live in `menu_strings/<code>.json`, one flat object per language, all with the same
//! keys as `en.json`:
//!
//! - `ui.<id>`: page strings (buttons, titles, messages). `{n}` is a count, `{name}` a setting
//!   label (the caller lower-cases it where the language has case).
//! - `label.<Mod>.<key>` and `desc.<Mod>.<key>`: setting label and one-sentence help. The mod is the
//!   folder name (`GameMod::name`). `label.*.<key>` and `desc.*.<key>` are the fallback for keys
//!   every mod has (`status`, `write_logs`).
//! - `choice.<Mod>.<key>.<choice key>`: text of one choice of an Enum setting.
//!
//! Every lookup falls back to English; an unknown language code behaves as English.
//! Regenerating keys: see the docstring of `dev/menu_strings_check.py`.

use std::collections::HashMap;
use std::sync::OnceLock;

type Map = HashMap<&'static str, &'static str>;

/// `(language code, json)` for every game language.
const FILES: &[(&str, &str)] = &[
    ("en", include_str!("menu_strings/en.json")),
    ("de", include_str!("menu_strings/de.json")),
    ("fr", include_str!("menu_strings/fr.json")),
    ("it", include_str!("menu_strings/it.json")),
    ("es", include_str!("menu_strings/es.json")),
    ("es-419", include_str!("menu_strings/es-419.json")),
    ("ru", include_str!("menu_strings/ru.json")),
    ("pl", include_str!("menu_strings/pl.json")),
    ("nl", include_str!("menu_strings/nl.json")),
    ("pt", include_str!("menu_strings/pt.json")),
    ("pt-BR", include_str!("menu_strings/pt-BR.json")),
    ("ja", include_str!("menu_strings/ja.json")),
    ("zh", include_str!("menu_strings/zh.json")),
    ("zh-Hant", include_str!("menu_strings/zh-Hant.json")),
    ("ko", include_str!("menu_strings/ko.json")),
];

fn parse(json: &str) -> Map {
    let parsed: HashMap<String, String> = serde_json::from_str(json).unwrap_or_default();
    // Leaked on purpose: the tables live for the whole run and callers get `&'static str`.
    parsed
        .into_iter()
        .map(|(k, v)| (&*Box::leak(k.into_boxed_str()), &*Box::leak(v.into_boxed_str())))
        .collect()
}

fn tables() -> &'static HashMap<&'static str, Map> {
    static TABLES: OnceLock<HashMap<&'static str, Map>> = OnceLock::new();
    TABLES.get_or_init(|| FILES.iter().map(|(code, json)| (*code, parse(json))).collect())
}

fn table(lang: &str) -> &'static Map {
    let t = tables();
    t.get(lang).or_else(|| t.get("en")).expect("en.json is built in")
}

fn english() -> &'static Map {
    &tables()["en"]
}

fn find(lang: &str, key: &str) -> Option<&'static str> {
    table(lang).get(key).or_else(|| english().get(key)).copied()
}

/// Page string `ui.<id>` in `lang`; English if missing there, `""` if unknown.
pub fn ui(lang: &str, id: &str) -> &'static str {
    find(lang, &format!("ui.{id}")).unwrap_or("")
}

fn setting(lang: &str, kind: &str, mod_name: &str, key: &str) -> Option<&'static str> {
    find(lang, &format!("{kind}.{mod_name}.{key}")).or_else(|| find(lang, &format!("{kind}.*.{key}")))
}

/// Label of a setting row.
pub fn label(lang: &str, mod_name: &str, key: &str) -> Option<&'static str> {
    setting(lang, "label", mod_name, key)
}

/// One-sentence help of a setting row.
pub fn desc(lang: &str, mod_name: &str, key: &str) -> Option<&'static str> {
    setting(lang, "desc", mod_name, key)
}

/// Text of one choice of an Enum setting.
pub fn choice(lang: &str, mod_name: &str, key: &str, choice_key: &str) -> Option<&'static str> {
    find(lang, &format!("choice.{mod_name}.{key}.{choice_key}"))
}

pub fn list_separator(lang: &str) -> &'static str {
    if matches!(lang, "ja" | "zh" | "zh-Hant") { "、" } else { ", " }
}

/// Replaces `{n}` and `{name}` in `text`.
pub fn fill(text: &str, n: usize, name: &str) -> String {
    text.replace("{n}", &n.to_string()).replace("{name}", name)
}

/// UTF-8 text in a zero-filled buffer for the game, cut at a character boundary, always
/// NUL-terminated (at most `N - 1` text bytes). CJK and Hangul take 3 bytes a character.
pub fn fixed<const N: usize>(text: &str) -> [u8; N] {
    let mut out = [0u8; N];
    let mut end = text.len().min(N.saturating_sub(1));
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    out[..end].copy_from_slice(&text.as_bytes()[..end]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn keys(m: &Map) -> BTreeSet<&'static str> {
        m.keys().copied().collect()
    }

    #[test]
    fn same_keys_as_english() {
        let en = keys(english());
        for (code, _) in FILES {
            let t = table(code);
            let k = keys(t);
            let missing: Vec<_> = en.difference(&k).collect();
            let extra: Vec<_> = k.difference(&en).collect();
            assert!(missing.is_empty() && extra.is_empty(), "{code}: missing {missing:?}, extra {extra:?}");
        }
    }

    #[test]
    fn every_file_is_valid_json_with_all_languages() {
        for (code, json) in FILES {
            let v: HashMap<String, String> = serde_json::from_str(json).unwrap_or_else(|e| panic!("{code}: {e}"));
            assert!(!v.is_empty(), "{code}");
        }
        assert_eq!(FILES.len(), 15);
    }

    #[test]
    fn every_language_names_every_never_spend_row() {
        let groups = crate::quick_recharge::fuel_groups();
        for (lang, _) in FILES {
            for fuel in &groups {
                let key = format!("choice.QuickRecharge.never_spend.{}", fuel.id);
                assert!(table(lang).contains_key(key.as_str()), "{lang} {key}");
            }
        }
    }

    #[test]
    fn no_empty_values_or_dashes() {
        for (code, _) in FILES {
            for (k, v) in table(code) {
                assert!(!v.trim().is_empty(), "{code} {k} is empty");
                assert!(!v.contains('\u{2014}') && !v.contains('\u{2013}'), "{code} {k} has a dash");
            }
        }
    }

    #[test]
    fn placeholders_survive() {
        for (code, _) in FILES {
            for (k, en) in english() {
                let v = table(code)[k];
                for p in ["{n}", "{name}"] {
                    assert_eq!(en.contains(p), v.contains(p), "{code} {k}: {p}");
                }
                assert!(!v.contains("{N}") && !v.contains("{Name}"), "{code} {k}");
            }
        }
    }

    #[test]
    fn lookups_and_fallbacks() {
        assert_eq!(ui("en", "title"), ui("xx", "title"));
        assert_eq!(ui("de", "nope"), "");
        assert_eq!(label("de", "HideEmptyHover", "write_logs"), label("de", "Other", "write_logs"));
        assert!(label("ja", "QuickRecharge", "never_spend").is_some());
        assert!(desc("fr", "CraftFromStorage", "anywhere").is_some());
        assert_eq!(choice("en", "QuickTransferAll", "key", "ctrl"), Some("Control"));
        assert!(choice("zh", "QuickTransferAll", "key", "ctrl").is_some());
        // The dev-only self-test has no strings here: its rows use the English spec labels.
        assert_eq!(label("en", "NMS.rs self-test", "stroke_log"), None);
        assert_eq!(label("en", "QuickRecharge", "nope"), None);
        assert_eq!(fill("{n} change(s)", 3, "x"), "3 change(s)");
        assert_eq!(fill("Clear {name}", 0, "log"), "Clear log");
    }

    #[test]
    fn english_matches_describe() {
        assert_eq!(
            desc("en", "HideEmptyHover", "hide_tech_hover"),
            crate::common::descriptions::describe("HideEmptyHover", "hide_tech_hover")
        );
    }
}

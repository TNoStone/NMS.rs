//! Per-mod settings in `GAMEDATA\MODS\<Mod>\<module>.json`, the same file and shape the Python
//! mods use, so a player's settings carry over.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

pub struct Settings {
    path: PathBuf,
    defaults: Map<String, Value>,
    values: Map<String, Value>,
}

impl Settings {
    /// Read the file, keep only known keys, fill the rest from `defaults`, and write it back if
    /// that changed anything. A file that cannot be parsed is left alone and the defaults are used.
    pub fn load(path: &Path, defaults: Value) -> Self {
        let defaults = match defaults {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let (data, readable) = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(map)) => (map, true),
                _ => (Map::new(), false),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Map::new(), true),
            Err(_) => (Map::new(), false),
        };
        let mut values = Map::new();
        for (key, default) in &defaults {
            values.insert(key.clone(), data.get(key).cloned().unwrap_or_else(|| default.clone()));
        }
        let settings = Self { path: path.to_path_buf(), defaults, values };
        if readable && settings.values != data {
            let _ = settings.save();
        }
        settings
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// The default passed to [`Settings::load`], whatever the value is now.
    pub fn default(&self, key: &str) -> Option<&Value> {
        self.defaults.get(key)
    }

    /// Python mods treat anything but an explicit `false` as on for default-on switches.
    pub fn is_not_false(&self, key: &str) -> bool {
        self.values.get(key) != Some(&Value::Bool(false))
    }

    pub fn is_true(&self, key: &str) -> bool {
        self.values.get(key) == Some(&Value::Bool(true))
    }

    /// Set a known key and save. Unknown keys are refused.
    pub fn set(&mut self, key: &str, value: Value) -> Result<(), String> {
        if !self.defaults.contains_key(key) {
            return Err(format!("unknown setting {key:?}"));
        }
        let previous = self.values.insert(key.to_string(), value);
        if let Err(err) = self.save() {
            if let Some(previous) = previous {
                self.values.insert(key.to_string(), previous);
            } else {
                self.values.remove(key);
            }
            return Err(err);
        }
        Ok(())
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&Value::Object(self.values.clone())).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, text + "\n").map_err(|e| e.to_string())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nms-core-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("mod.json")
    }

    #[test]
    fn writes_defaults_when_missing() {
        let path = temp("missing");
        let s = Settings::load(&path, json!({"write_logs": false, "refiner_tab": true}));
        assert!(s.is_not_false("refiner_tab"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\n  \"write_logs\": false,\n  \"refiner_tab\": true\n}\n");
    }

    #[test]
    fn keeps_known_keys_and_drops_unknown() {
        let path = temp("merge");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"refiner_tab": false, "old_key": 1}"#).unwrap();
        let s = Settings::load(&path, json!({"write_logs": false, "refiner_tab": true}));
        assert!(!s.is_not_false("refiner_tab"));
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved, json!({"write_logs": false, "refiner_tab": false}));
    }

    #[test]
    fn leaves_broken_file_alone() {
        let path = temp("broken");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let s = Settings::load(&path, json!({"refiner_tab": true}));
        assert!(s.is_true("refiner_tab"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn default_survives_set() {
        let path = temp("default");
        let mut s = Settings::load(&path, json!({"n": 5}));
        s.set("n", json!(9)).unwrap();
        assert_eq!(s.get("n"), Some(&json!(9)));
        assert_eq!(s.default("n"), Some(&json!(5)));
        assert_eq!(s.default("missing"), None);
    }

    #[test]
    fn refuses_unknown_key() {
        let path = temp("unknown");
        let mut s = Settings::load(&path, json!({"a": true}));
        assert!(s.set("b", json!(true)).is_err());
        assert!(s.set("a", json!(false)).is_ok());
        assert!(!s.is_true("a"));
    }

    #[test]
    fn failed_save_keeps_the_previous_value_in_memory() {
        let path = temp("save-fail");
        let mut settings = Settings::load(&path, json!({"a": true}));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(settings.set("a", json!(false)).is_err());
        assert!(settings.is_true("a"));
        std::fs::remove_dir(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
    }
}

//! Per-user settings and the recent-file list, stored as one JSON file.
//!
//! Writes go through a temp file plus rename: an archiver that dies mid-write
//! should not leave a half-written settings file behind.  The `*_in` variants
//! take an explicit directory because the environment-variable route is
//! process-global and made the tests race each other.

use crate::formats::Format;
use crate::models::Overwrite;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const HISTORY_LIMIT: usize = 12;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// "dark" | "light" | "system"
    pub theme: String,
    /// "material" | "fluent"
    pub skin: String,
    pub default_format: Format,
    /// 0..=9
    pub level: u8,
    /// 0 means "let the engine decide"
    pub threads: u8,
    pub overwrite: Overwrite,
    pub repair_names: bool,
    pub open_after_extract: bool,
    pub use_recycle_bin: bool,
    pub history: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            skin: "material".into(),
            default_format: Format::SevenZip,
            level: 5,
            threads: 0,
            overwrite: Overwrite::KeepBoth,
            repair_names: true,
            open_after_extract: true,
            use_recycle_bin: true,
            history: Vec::new(),
        }
    }
}

impl Settings {
    /// `%APPDATA%/QuickArc` on Windows, XDG config elsewhere.  A `QARC_CONFIG_DIR`
    /// override exists for portable runs and for the settings dialog's tests.
    pub fn config_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("QARC_CONFIG_DIR") {
            return PathBuf::from(dir);
        }
        if cfg!(windows) {
            if let Some(appdata) = std::env::var_os("APPDATA") {
                return Path::new(&appdata).join("QuickArc");
            }
        }
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return Path::new(&xdg).join("quickarc");
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        match home {
            Some(h) => Path::new(&h).join(".config").join("quickarc"),
            None => PathBuf::from("quickarc-config"),
        }
    }

    pub fn file_in(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    pub fn path() -> PathBuf {
        Self::file_in(&Self::config_dir())
    }

    /// Never fails: a missing or corrupt file falls back to defaults, because a
    /// first-run user must not be blocked by a settings problem.
    pub fn load_in(dir: &Path) -> Settings {
        std::fs::read_to_string(Self::file_in(dir))
            .ok()
            .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Settings {
        Self::load_in(&Self::config_dir())
    }

    pub fn save_to(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let target = Self::file_in(dir);
        let tmp = target.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        // Windows refuses to rename onto an existing file.
        if target.exists() {
            std::fs::remove_file(&target).map_err(|e| e.to_string())?;
        }
        std::fs::rename(&tmp, &target).map_err(|e| e.to_string())
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::config_dir())
    }

    /// Most-recent-first, de-duplicated, capped.
    pub fn record(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        self.history.retain(|p| !p.eq_ignore_ascii_case(path));
        self.history.insert(0, path.to_string());
        self.history.truncate(HISTORY_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new(tag: &str) -> Sandbox {
            let mut root = std::env::temp_dir();
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            root.push(format!("qarc-cfg-{tag}-{stamp}-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            Sandbox(root)
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn settings_round_trip_keeps_every_field() {
        let dir = Sandbox::new("round");
        let mut s = Settings::default();
        s.theme = "light".into();
        s.skin = "fluent".into();
        s.default_format = Format::TarGz;
        s.level = 9;
        s.threads = 8;
        s.overwrite = Overwrite::Skip;
        s.repair_names = false;
        s.use_recycle_bin = false;
        s.save_to(&dir.0).unwrap();

        let back = Settings::load_in(&dir.0);
        assert_eq!(back.theme, "light");
        assert_eq!(back.skin, "fluent");
        assert_eq!(back.default_format, Format::TarGz);
        assert_eq!(back.level, 9);
        assert_eq!(back.threads, 8);
        assert_eq!(back.overwrite, Overwrite::Skip);
        assert!(!back.repair_names);
        assert!(!back.use_recycle_bin);
    }

    #[test]
    fn corrupt_file_and_missing_file_both_fall_back_to_defaults() {
        let dir = Sandbox::new("corrupt");
        std::fs::write(Settings::file_in(&dir.0), "{ not json").unwrap();
        assert_eq!(Settings::load_in(&dir.0).theme, "system");
        std::fs::remove_file(Settings::file_in(&dir.0)).unwrap();
        assert_eq!(Settings::load_in(&dir.0).level, 5);
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let dir = Sandbox::new("atomic");
        Settings::default().save_to(&dir.0).unwrap();
        Settings::default().save_to(&dir.0).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn history_is_deduplicated_capped_and_most_recent_first() {
        let dir = Sandbox::new("history");
        let mut s = Settings::default();
        for i in 0..20 {
            s.record(&format!("D:/a/archive{i}.7z"));
        }
        assert_eq!(s.history.len(), HISTORY_LIMIT);
        assert_eq!(s.history[0], "D:/a/archive19.7z");

        s.record("D:/a/archive5.7z");
        assert_eq!(s.history[0], "D:/a/archive5.7z");
        assert_eq!(s.history.iter().filter(|p| *p == "D:/a/archive5.7z").count(), 1);

        s.record("");
        assert_eq!(s.history[0], "D:/a/archive5.7z");
        s.save_to(&dir.0).unwrap();
        assert_eq!(Settings::load_in(&dir.0).history, s.history);
    }

    #[test]
    fn enums_are_stored_by_name_not_index() {
        // The UI shows these strings, so a renumbered enum must not silently
        // reinterpret somebody's saved setting.
        let dir = Sandbox::new("names");
        let mut s = Settings::default();
        s.overwrite = Overwrite::Newer;
        s.save_to(&dir.0).unwrap();
        let text = std::fs::read_to_string(Settings::file_in(&dir.0)).unwrap();
        assert!(text.contains("Newer"), "{text}");
        assert!(text.contains("SevenZip"), "{text}");
        assert!(!text.contains(": 4,"), "numbers would silently shift meanings: {text}");
    }

    #[test]
    fn defaults_match_what_the_settings_dialog_shows_first_time() {
        let s = Settings::default();
        assert_eq!(s.theme, "system");
        assert_eq!(s.skin, "material");
        assert_eq!(s.level, 5);
        assert!(s.repair_names, "legacy Chinese names are repaired by default");
        assert!(s.use_recycle_bin, "delete goes to the bin unless the user says otherwise");
        assert_eq!(s.overwrite, Overwrite::KeepBoth, "never overwrite silently");
    }
}

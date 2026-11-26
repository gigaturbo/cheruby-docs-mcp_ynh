//! The settings the page changes: the index schedule, its quiet hours and
//! each source's filters. Kept in a JSON file the app owns
//! (DOCS_SETTINGS_FILE, /var/lib/cheruby-docs-mcp/settings.json), backed up.
//!
//! Nothing here confines the app: the user, the folders, writable, the MCP token's
//! folders, the Nextcloud login, the tokens and the network stay in the
//! YunoHost panel, root's. A filter hides files from the agents, but the
//! process that changes it reads those folders anyway.

use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};

use crate::{
    calendar::{self, Cron},
    folders::{Rules, UserFolders},
};

/// Where a folder comes from, as the panel's sections name them.
pub const SOURCES: [&str; 3] = ["syncthing", "local", "nextcloud"];
const PRESETS: [(&str, &str); 3] = [
    ("hourly", "20 * * * *"),
    ("6h", "20 */6 * * *"),
    ("nightly", "20 23 * * *"),
];
const MAX_CRON_CHARS: usize = 100;
const MAX_PATTERN_CHARS: usize = 200;
const MAX_PATTERNS: usize = 100;
/// Far above what the page can save; a bigger file is not read.
const MAX_FILE_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// hourly, 6h, nightly, custom (cron below) or never.
    pub schedule: String,
    pub cron: String,
    /// Such as 01-06: no run starts then, and one going stops. Empty for none.
    pub quiet: String,
    /// By source: what a folder of that source keeps and leaves out.
    pub filters: BTreeMap<String, Filter>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Filter {
    pub only: Vec<String>,
    pub skip: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schedule: "hourly".into(),
            cron: String::new(),
            // Around the nightly backups, 01:30 to about 05:45.
            quiet: "01-06".into(),
            filters: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// The cron line the schedule runs on; None when it is never.
    pub fn cron(&self) -> Option<Cron> {
        let line = match self.schedule.as_str() {
            "custom" => self.cron.as_str(),
            name => PRESETS.iter().find(|(n, _)| *n == name)?.1,
        };
        Cron::parse(line)
    }

    /// The next scheduled run after t, outside the quiet hours.
    pub fn next(&self, t: NaiveDateTime) -> Option<NaiveDateTime> {
        self.cron()?.next(&self.quiet, t)
    }

    /// Why these settings cannot be used, in words for the page.
    pub fn check(&self) -> Result<(), String> {
        let known = self.schedule == "custom"
            || self.schedule == "never"
            || PRESETS.iter().any(|(n, _)| *n == self.schedule);
        if !known {
            return Err("The schedule is hourly, 6h, nightly, custom or never.".into());
        }
        if self.schedule == "custom"
            && (self.cron.chars().count() > MAX_CRON_CHARS || Cron::parse(&self.cron).is_none())
        {
            return Err("Not a cron schedule: minute 0-59, hour 0-23, day 1-31, month 1-12, weekday 0-7 or sun-sat, and not both a day and a weekday. Such as 20 6-23 * * *.".into());
        }
        if !calendar::quiet_ok(&self.quiet) {
            return Err("The quiet hours are two different hours, such as 01-06, or empty.".into());
        }
        if self.schedule != "never" && self.next(chrono::Local::now().naive_local()).is_none() {
            return Err("The quiet hours leave this schedule no time to run.".into());
        }
        for (source, filter) in &self.filters {
            if !SOURCES.contains(&source.as_str()) {
                return Err(format!("No source {source}."));
            }
            for list in [&filter.only, &filter.skip] {
                if list.len() > MAX_PATTERNS {
                    return Err(format!("{MAX_PATTERNS} patterns at most in a list."));
                }
                if let Some(bad) = list.iter().find(|p| {
                    p.trim_matches('/').trim().is_empty()
                        || p.chars().count() > MAX_PATTERN_CHARS
                        || p.chars().any(char::is_control)
                }) {
                    return Err(format!(
                        "\"{}\" is not a pattern: up to {MAX_PATTERN_CHARS} characters, with no line break.",
                        bad.escape_default()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Each folder's rules, from the filters of its source.
    pub fn apply(&self, own: &mut UserFolders) {
        let patterns = |list: &[String]| -> Vec<String> {
            list.iter()
                .map(|p| p.to_lowercase().trim_matches('/').to_string())
                .filter(|p| !p.is_empty())
                .collect()
        };
        let mut rules = HashMap::new();
        for (label, _) in &own.roots {
            let filter = own
                .sources
                .get(label)
                .and_then(|s| self.filters.get(s))
                .cloned()
                .unwrap_or_default();
            rules.insert(
                label.clone(),
                Rules {
                    only: patterns(&filter.only),
                    skip: patterns(&filter.skip),
                },
            );
        }
        own.rules = rules;
    }

    /// The settings file, or the defaults when there is none yet. The status
    /// command reads it as root, and the tool server owns its folder: never
    /// through a link, never a pipe that would hang the reader, never past
    /// MAX_FILE_BYTES.
    pub fn load(path: &Path) -> Result<Settings, String> {
        use nix::fcntl::OFlag;
        let shown = path.display();
        let mut text = String::new();
        let f = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
            .open(path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
            Err(e) => return Err(format!("{shown}: {e}")),
        };
        if !f.metadata().is_ok_and(|m| m.is_file()) {
            return Err(format!("{shown}: not a regular file."));
        }
        f.take(MAX_FILE_BYTES)
            .read_to_string(&mut text)
            .map_err(|e| format!("{shown}: {e}"))?;
        let settings: Settings =
            serde_json::from_str(&text).map_err(|e| format!("{shown}: {e}"))?;
        settings.check().map_err(|e| format!("{shown}: {e}"))?;
        Ok(settings)
    }

    /// DOCS_SETTINGS_FILE's settings; the defaults when it is not set.
    pub fn from_env() -> Result<Settings, String> {
        match std::env::var_os("DOCS_SETTINGS_FILE") {
            Some(path) => Settings::load(Path::new(&path)),
            None => Ok(Settings::default()),
        }
    }

    /// Written whole beside the file, then renamed over it, so a reader
    /// never sees half of it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let shown = path.display();
        let dir = path.parent().ok_or(format!("{shown}: no folder."))?;
        let name = path
            .file_name()
            .ok_or(format!("{shown}: no file name."))?
            .to_string_lossy();
        let tmp = dir.join(format!(".{name}.tmp"));
        let _ = std::fs::remove_file(&tmp);
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("{shown}: {e}"))?;
        f.write_all(text.as_bytes())
            .and_then(|_| f.sync_all())
            .map_err(|e| format!("{shown}: {e}"))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{shown}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filtered(only: &[&str], skip: &[&str]) -> Settings {
        let s = |v: &[&str]| v.iter().map(|p| p.to_string()).collect();
        Settings {
            filters: BTreeMap::from([(
                "local".to_string(),
                Filter {
                    only: s(only),
                    skip: s(skip),
                },
            )]),
            ..Settings::default()
        }
    }

    #[test]
    fn what_the_page_refuses() {
        assert!(Settings::default().check().is_ok());
        let refused = |s: Settings| s.check().is_err();
        assert!(refused(Settings {
            schedule: "weekly".into(),
            ..Settings::default()
        }));
        assert!(refused(Settings {
            schedule: "custom".into(),
            cron: "0 3 1 * 1".into(),
            ..Settings::default()
        }));
        assert!(refused(Settings {
            schedule: "custom".into(),
            cron: "0 3 * * *".into(),
            ..Settings::default()
        }));
        assert!(refused(Settings {
            quiet: "06-06".into(),
            ..Settings::default()
        }));
        assert!(
            Settings {
                schedule: "never".into(),
                quiet: "00-23".into(),
                ..Settings::default()
            }
            .check()
            .is_ok()
        );
        assert!(refused(filtered(&["/"], &[])));
        assert!(refused(filtered(&[], &["a\nb"])));
        let mut other = filtered(&[], &[]);
        other.filters.insert("ftp".into(), Filter::default());
        assert!(refused(other));
        assert!(
            filtered(&["*.pdf", "Documents/Factures"], &["Old"])
                .check()
                .is_ok()
        );
    }

    #[test]
    fn rules_follow_each_folders_source() {
        let mut own = UserFolders {
            roots: vec![
                ("Notes".into(), "/n".into()),
                ("Cloud".into(), "nextcloud:".into()),
            ],
            sources: HashMap::from([
                ("Notes".to_string(), "local".to_string()),
                ("Cloud".to_string(), "nextcloud".to_string()),
            ]),
            ..UserFolders::default()
        };
        filtered(&["*.MD"], &["/Notes/Old/"]).apply(&mut own);
        assert_eq!(own.rules["Notes"].only, ["*.md"]);
        assert_eq!(own.rules["Notes"].skip, ["notes/old"]);
        assert!(own.rules["Cloud"].only.is_empty() && own.rules["Cloud"].skip.is_empty());
    }

    #[test]
    fn saved_and_read_back() {
        let dir = std::env::temp_dir().join(format!("settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        assert_eq!(Settings::load(&path).unwrap(), Settings::default());
        let s = filtered(&["*.pdf"], &[]);
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path).unwrap(), s);
        std::os::unix::fs::symlink(&path, dir.join("link.json")).unwrap();
        assert!(Settings::load(&dir.join("link.json")).is_err());
        nix::unistd::mkfifo(&dir.join("pipe.json"), nix::sys::stat::Mode::S_IRWXU).unwrap();
        assert!(Settings::load(&dir.join("pipe.json")).is_err());
        std::fs::write(&path, r#"{"schedule": "hourly", "extra": 1}"#).unwrap();
        assert!(Settings::load(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

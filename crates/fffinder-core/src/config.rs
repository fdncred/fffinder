//! Settings shared by the GUI and the Nushell plugin, stored as JSON in the
//! platform config directory (`~/Library/Application Support/fffinder` on
//! macOS, `~/.config/fffinder` on Linux, `%APPDATA%\fffinder` on Windows).

use crate::grouping::GroupKey;
use crate::kind::KindFilter;
use crate::listing::{SortKey, SortOrder};
use crate::terminals::{CUSTOM, Platform, TerminalSetting};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "fffinder";
/// Preview text size, in points, before the user changes it.
pub const DEFAULT_PREVIEW_FONT_SIZE: f32 = 11.5;
/// Text size of the Nushell panel and action menu, in points.
pub const DEFAULT_NU_FONT_SIZE: f32 = 12.0;
/// The smallest and largest preview and Nushell text sizes.
pub const FONT_SIZE_RANGE: (f32, f32) = (8.0, 32.0);
/// The smallest and largest scale of the whole window's text and controls.
pub const UI_SCALE_RANGE: (f32, f32) = (0.8, 2.0);
/// The app's earlier name. Its folders are moved on first start.
const LEGACY_NAME: &str = "fff-Finder";

/// The environment variable `name` (`FFFINDER_…`), or the same variable under
/// the earlier name (`FFF_FINDER_…`). Empty counts as unset.
pub fn env_var(name: &str) -> Option<OsString> {
    let rest = name.strip_prefix("FFFINDER_").unwrap_or(name);
    [name.to_string(), format!("FFF_FINDER_{rest}")]
        .into_iter()
        .find_map(std::env::var_os)
        .filter(|v| !v.is_empty())
}

fn env_dir(name: &str) -> Option<PathBuf> {
    env_var(name).map(PathBuf::from)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Icons,
    List,
    Columns,
    Gallery,
}

impl ViewMode {
    pub const ALL: [ViewMode; 4] = [
        ViewMode::Icons,
        ViewMode::List,
        ViewMode::Columns,
        ViewMode::Gallery,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ViewMode::Icons => "Icons",
            ViewMode::List => "List",
            ViewMode::Columns => "Columns",
            ViewMode::Gallery => "Gallery",
        }
    }
}

/// A column the list view can show after Name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListColumn {
    Modified,
    Created,
    LastOpened,
    Size,
    Kind,
    Permissions,
    Git,
}

impl ListColumn {
    pub const ALL: [ListColumn; 7] = [
        ListColumn::Modified,
        ListColumn::Created,
        ListColumn::LastOpened,
        ListColumn::Size,
        ListColumn::Kind,
        ListColumn::Permissions,
        ListColumn::Git,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ListColumn::Modified => "Date Modified",
            ListColumn::Created => "Date Created",
            ListColumn::LastOpened => "Date Last Opened",
            ListColumn::Size => "Size",
            ListColumn::Kind => "Kind",
            ListColumn::Permissions => "Permissions",
            ListColumn::Git => "Git",
        }
    }

    /// Width in points before the user drags it.
    pub fn default_width(self) -> f32 {
        match self {
            ListColumn::Modified | ListColumn::Created | ListColumn::LastOpened => 170.,
            ListColumn::Size | ListColumn::Git => 90.,
            ListColumn::Kind => 160.,
            ListColumn::Permissions => 100.,
        }
    }

    /// What clicking the column's header sorts by, if anything.
    pub fn sort_key(self) -> Option<SortKey> {
        match self {
            ListColumn::Modified => Some(SortKey::Modified),
            ListColumn::Created => Some(SortKey::Created),
            ListColumn::LastOpened => Some(SortKey::LastOpened),
            ListColumn::Size => Some(SortKey::Size),
            ListColumn::Kind => Some(SortKey::Kind),
            ListColumn::Permissions | ListColumn::Git => None,
        }
    }
}

/// One of the list view's columns, in order, and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColumnSetting {
    pub column: ListColumn,
    pub width: f32,
}

/// The narrowest a list column can be dragged.
pub const MIN_COLUMN_WIDTH: f32 = 50.;

pub fn default_list_columns() -> Vec<ColumnSetting> {
    [ListColumn::Modified, ListColumn::Size, ListColumn::Kind]
        .into_iter()
        .map(|column| ColumnSetting {
            column,
            width: column.default_width(),
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Folders indexed for "This Computer" searches.
    pub search_roots: Vec<PathBuf>,
    /// Sidebar favorites, in display order.
    pub favorites: Vec<PathBuf>,
    pub show_hidden: bool,
    pub view_mode: ViewMode,
    pub sort: SortOrder,
    /// Folders before files, whatever the sort ("Keep Folders on Top").
    pub folders_first: bool,
    /// Show the list and icon views in groups ("Use Groups"), by `group_by`.
    pub use_groups: bool,
    pub group_by: GroupKey,
    /// The list view's columns after Name, in order, with their widths.
    pub list_columns: Vec<ColumnSetting>,
    /// Show folder sizes, added up from the index ("Calculate All Sizes").
    pub folder_sizes: bool,
    /// Servers connected to with Connect to Server, newest first.
    pub recent_servers: Vec<String>,
    pub icon_size: f32,
    pub show_sidebar: bool,
    pub appearance: Appearance,
    pub kind_filter: KindFilter,
    /// Maximum results shown for a search.
    pub result_limit: usize,
    /// The terminal "Open Nushell Here" uses, per operating system (`macos`,
    /// `linux`, `windows`), so one settings file can carry a choice for each.
    /// A missing entry picks one automatically.
    pub terminals: BTreeMap<String, TerminalSetting>,
    /// Saved searches shown in the sidebar.
    pub smart_folders: Vec<SmartFolder>,
    /// Color names with Nushell's `$env.LS_COLORS` and open files in its
    /// `buffer_editor`, read from `nu --login`.
    pub use_nu_environment: bool,
    /// Text size of text, code, and table previews in Quick Look and the
    /// gallery; Quick Look's buttons and Cmd+Plus/Minus change it.
    pub preview_font_size: f32,
    /// Text size of the Nushell pipeline panel and the "Nushell Actions" menu.
    pub nu_font_size: f32,
    /// Scale of all text and controls in the window; 1.0 is normal.
    pub ui_scale: f32,
    /// fff-search's indexing and search options.
    pub search: SearchSettings,
    /// The older single terminal command; moved into `terminals` on load.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Path to `nu`; `None` searches `PATH`.
    pub nu_path: Option<PathBuf>,
    pub last_folder: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            search_roots: default_search_roots(),
            favorites: crate::places::default_favorites()
                .into_iter()
                .map(|p| p.path)
                .collect(),
            show_hidden: false,
            view_mode: ViewMode::Icons,
            sort: SortOrder::default(),
            folders_first: false,
            use_groups: false,
            group_by: GroupKey::default(),
            list_columns: default_list_columns(),
            folder_sizes: true,
            recent_servers: Vec::new(),
            icon_size: 64.0,
            show_sidebar: true,
            appearance: Appearance::System,
            kind_filter: KindFilter::Any,
            result_limit: 500,
            terminals: BTreeMap::new(),
            smart_folders: Vec::new(),
            use_nu_environment: false,
            preview_font_size: DEFAULT_PREVIEW_FONT_SIZE,
            nu_font_size: DEFAULT_NU_FONT_SIZE,
            ui_scale: 1.0,
            search: SearchSettings::default(),
            terminal: None,
            nu_path: None,
            last_folder: None,
        }
    }
}

/// fff-search's options, from Settings › Search Engine. The first five shape
/// the indexes, so changing one rebuilds them; the rest apply to each search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchSettings {
    /// Follow symbolic links while indexing.
    pub follow_symlinks: bool,
    /// Keep indexes current with filesystem watchers.
    pub watch: bool,
    /// Rank files changed in the last commits of a git repository higher.
    pub git_recency: bool,
    /// After scanning, index file contents so content searches skip
    /// non-matching files sooner.
    pub content_index: bool,
    /// Keep the most-used files in memory for content search.
    pub preload_files: bool,
    /// Letter case in content search.
    pub case: SearchCase,
    /// Content search skips files bigger than this, in megabytes.
    pub max_file_size_mb: u64,
    /// Content search returns what it found after this many seconds;
    /// 0 waits for everything.
    pub time_limit_secs: u64,
    /// Threads for name searches; 0 uses one per core.
    pub threads: usize,
}

impl Default for SearchSettings {
    fn default() -> Self {
        SearchSettings {
            follow_symlinks: false,
            watch: true,
            git_recency: true,
            content_index: false,
            preload_files: false,
            case: SearchCase::Smart,
            max_file_size_mb: 10,
            time_limit_secs: 5,
            threads: 0,
        }
    }
}

impl SearchSettings {
    /// The options an index is built with; when these change, the indexes
    /// are rebuilt.
    pub fn index_options(&self) -> [bool; 5] {
        [
            self.follow_symlinks,
            self.watch,
            self.git_recency,
            self.content_index,
            self.preload_files,
        ]
    }
}

/// Letter case in content search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchCase {
    /// Ignore case unless the text has a capital letter.
    #[default]
    Smart,
    Sensitive,
    Insensitive,
}

/// A saved search: what to look for, where, and a Nushell filter over the
/// results (sizes, dates, anything `where` can test).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SmartFolder {
    pub name: String,
    pub query: String,
    pub kind: KindFilter,
    /// Only inside this folder; `None` searches every location.
    pub scope: Option<PathBuf>,
    /// Search file contents instead of names.
    pub contents: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
}

/// `value` clamped to `(min, max)`, or `default` when it isn't a number.
pub fn in_range(value: f32, (min, max): (f32, f32), default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        default
    }
}

/// Home, plus the system application folder on macOS.
pub fn default_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home);
    }
    #[cfg(target_os = "macos")]
    roots.push(PathBuf::from("/Applications"));
    roots
}

/// Settings directory; `FFFINDER_CONFIG_DIR` overrides it (used by tests).
pub fn config_dir() -> PathBuf {
    env_dir("FFFINDER_CONFIG_DIR").unwrap_or_else(|| {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_NAME)
    })
}

/// Where frecency databases, cached icons, and the control socket live;
/// `FFFINDER_DATA_DIR` overrides it (used by tests).
pub fn data_dir() -> PathBuf {
    env_dir("FFFINDER_DATA_DIR").unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_NAME)
    })
}

/// Move the folders the app used under an earlier name to the new name, once.
/// Overridden folders are left alone.
pub fn migrate_legacy_dirs() {
    let pairs = [
        ("FFFINDER_CONFIG_DIR", dirs::config_dir()),
        ("FFFINDER_DATA_DIR", dirs::data_local_dir()),
    ];
    for (var, base) in pairs {
        if let Some(base) = base.filter(|_| env_dir(var).is_none()) {
            migrate_legacy_dir(&base);
        }
    }
}

/// Rename the earlier-named folder in `base` to [`APP_NAME`], unless that
/// folder already exists.
fn migrate_legacy_dir(base: &Path) {
    let old = base.join(LEGACY_NAME);
    let new = base.join(APP_NAME);
    if old.is_dir()
        && !new.exists()
        && let Err(err) = std::fs::rename(&old, &new)
    {
        log::warn!("moving {} to {}: {err}", old.display(), new.display());
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("settings.json")
}

impl Config {
    /// Load settings; missing or malformed files fall back to defaults.
    pub fn load() -> Config {
        Self::load_from(&config_path())
    }

    pub fn load_from(path: &Path) -> Config {
        let mut config: Config = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if let Some(command) = config.terminal.take().filter(|c| !c.trim().is_empty()) {
            config
                .terminals
                .entry(Platform::current().key().into())
                .or_insert(TerminalSetting {
                    app: Some(CUSTOM.into()),
                    command: Some(command),
                });
        }
        config
    }

    /// The preview text size, kept readable even if the file was edited by hand.
    pub fn preview_font_size(&self) -> f32 {
        in_range(
            self.preview_font_size,
            FONT_SIZE_RANGE,
            DEFAULT_PREVIEW_FONT_SIZE,
        )
    }

    /// The Nushell panel's text size, kept in range like the preview's.
    pub fn nu_font_size(&self) -> f32 {
        in_range(self.nu_font_size, FONT_SIZE_RANGE, DEFAULT_NU_FONT_SIZE)
    }

    /// The window's scale, kept in range like the text sizes.
    pub fn ui_scale(&self) -> f32 {
        in_range(self.ui_scale, UI_SCALE_RANGE, 1.0)
    }

    /// This operating system's terminal setting.
    pub fn terminal_setting(&self) -> TerminalSetting {
        self.terminals
            .get(Platform::current().key())
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_terminal_setting(&mut self, setting: TerminalSetting) {
        let key = Platform::current().key().to_string();
        if setting == TerminalSetting::default() {
            self.terminals.remove(&key);
        } else {
            self.terminals.insert(key, setting);
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&config_path())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write then rename, so a crash never leaves a truncated file.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let path = std::env::temp_dir().join(format!("fffinder-cfg-{}.json", std::process::id()));
        let mut cfg = Config {
            show_hidden: true,
            view_mode: ViewMode::Gallery,
            preview_font_size: 16.5,
            nu_font_size: 14.0,
            ui_scale: 1.2,
            search: SearchSettings {
                follow_symlinks: true,
                case: SearchCase::Insensitive,
                time_limit_secs: 0,
                ..Default::default()
            },
            ..Default::default()
        };
        cfg.terminals.insert(
            "windows".into(),
            TerminalSetting {
                app: Some("windows-terminal".into()),
                command: None,
            },
        );
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path), cfg);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn moves_the_legacy_folder_once() {
        let base = std::env::temp_dir().join(format!("fffinder-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let old = base.join(LEGACY_NAME);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), "first").unwrap();
        let settings = base.join(APP_NAME).join("settings.json");

        migrate_legacy_dir(&base);
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "first");
        assert!(!old.exists());

        // The new folder exists now, so a later old folder stays where it is.
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("settings.json"), "second").unwrap();
        migrate_legacy_dir(&base);
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), "first");
        assert!(old.is_dir());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn keeps_a_terminal_per_system_and_moves_the_old_command() {
        let path = std::env::temp_dir().join(format!("fffinder-cfg3-{}.json", std::process::id()));
        std::fs::write(
            &path,
            r#"{"terminal": "foot -D {dir} {nu}", "terminals": {"plan9": {"app": "rio"}}}"#,
        )
        .unwrap();
        let mut cfg = Config::load_from(&path);
        assert_eq!(cfg.terminal, None);
        assert_eq!(
            cfg.terminal_setting(),
            TerminalSetting {
                app: Some(CUSTOM.into()),
                command: Some("foot -D {dir} {nu}".into())
            }
        );
        cfg.set_terminal_setting(TerminalSetting {
            app: Some("ghostty".into()),
            command: Some("foot -D {dir} {nu}".into()),
        });
        cfg.save_to(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("\"terminal\""), "{text}");
        assert!(text.contains("\"plan9\""), "other systems are kept: {text}");
        assert_eq!(
            Config::load_from(&path).terminal_setting().app.as_deref(),
            Some("ghostty")
        );
        cfg.set_terminal_setting(TerminalSetting::default());
        assert!(!cfg.terminals.contains_key(Platform::current().key()));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn tolerates_partial_files() {
        let path = std::env::temp_dir().join(format!("fffinder-cfg2-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"show_hidden": true}"#).unwrap();
        let cfg = Config::load_from(&path);
        assert!(cfg.show_hidden);
        assert_eq!(cfg.result_limit, 500);
        assert_eq!(cfg.preview_font_size(), DEFAULT_PREVIEW_FONT_SIZE);
        assert_eq!(cfg.nu_font_size(), DEFAULT_NU_FONT_SIZE);
        assert_eq!(cfg.ui_scale(), 1.0);
        std::fs::write(
            &path,
            r#"{"preview_font_size": 0.5, "nu_font_size": 99, "ui_scale": 0.1}"#,
        )
        .unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.preview_font_size(), 8.0);
        assert_eq!(cfg.nu_font_size(), 32.0);
        assert_eq!(cfg.ui_scale(), 0.8);
        assert_eq!(cfg.search, SearchSettings::default());
        std::fs::write(&path, r#"{"search": {"case": "sensitive", "threads": 4}}"#).unwrap();
        let cfg = Config::load_from(&path);
        assert_eq!(cfg.search.case, SearchCase::Sensitive);
        assert_eq!(cfg.search.threads, 4);
        assert!(cfg.search.watch, "missing fields keep their defaults");
        std::fs::remove_file(path).unwrap();
    }
}

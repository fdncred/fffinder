//! Core of fffinder: search engine wrapper around `fff-search`, the app list,
//! file model, folder listing, copying and moving, sidebar places, settings, terminals,
//! and formatting.

pub mod app_list;
pub mod archive;
pub mod config;
pub mod desktop_apps;
pub mod engine;
pub mod entry;
pub mod format;
pub mod grouping;
pub mod info;
pub mod kind;
pub mod listing;
pub mod media;
pub mod ops;
pub mod pdf;
pub mod places;
pub mod servers;
pub mod terminals;
pub mod thumbnailers;

pub use config::{
    Appearance, ColumnSetting, Config, ListColumn, SearchCase, SearchSettings, SmartFolder,
    ViewMode,
};
pub use engine::{
    Engine, EngineOptions, FolderStats, GrepHit, GrepModeKind, GrepRequest, GrepResponse,
    IndexedItem, RootStatus, SearchHit, SearchRequest, SearchResponse, Watch,
};
pub use entry::FileEntry;
pub use grouping::GroupKey;
pub use kind::{FileKind, KindFilter};
pub use listing::{SortKey, SortOrder};

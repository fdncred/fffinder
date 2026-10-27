//! Core of fffinder: search engine wrapper around `fff-search`, file model,
//! folder listing, copying and moving, sidebar places, settings, terminals,
//! and formatting.

pub mod config;
pub mod engine;
pub mod entry;
pub mod format;
pub mod kind;
pub mod listing;
pub mod ops;
pub mod places;
pub mod terminals;

pub use config::{Appearance, Config, SearchCase, SearchSettings, SmartFolder, ViewMode};
pub use engine::{
    Engine, EngineOptions, GrepHit, GrepModeKind, GrepRequest, GrepResponse, IndexedItem,
    RootStatus, SearchHit, SearchRequest, SearchResponse, Watch,
};
pub use entry::FileEntry;
pub use kind::{FileKind, KindFilter};
pub use listing::{SortKey, SortOrder};

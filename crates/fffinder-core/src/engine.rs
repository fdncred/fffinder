//! Search engine: one `fff-search` index per search root, searched in
//! parallel and merged by score. Folders outside every root get a temporary
//! index on demand, so "search this folder" works anywhere. Apps come from
//! an [`AppList`] instead of an index.

use crate::app_list::AppList;
use crate::config::{SearchCase, SearchSettings};
use crate::entry::{FileEntry, display_name};
use crate::kind::{self, KindFilter};
use fff_search::file_picker::FilePicker;
use fff_search::{
    Casing, DirSearchConfig, FFFMode, FilePickerOptions, FrecencyTracker, FuzzySearchOptions,
    GitRecencyConfig, GrepConfig, GrepMode, GrepSearchOptions, MixedItemRef, MixedSearchConfig,
    PaginationArgs, QueryParser, QueryTracker, SharedFilePicker, SharedFrecency,
    SharedQueryTracker, WatchEvent, WatchId, WatchOptions,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// Temporary (outside-of-roots) indexes kept alive, most recent first.
const MAX_ADHOC: usize = 3;

#[derive(Debug, Clone)]
pub struct EngineOptions {
    pub roots: Vec<PathBuf>,
    /// Folders whose apps "This Computer" searches find by name without
    /// indexing them (see [`AppList`]); empty for none.
    pub app_dirs: Vec<PathBuf>,
    /// Directory for frecency/query-history databases; `None` disables them.
    pub db_dir: Option<PathBuf>,
    /// Keep indexes live with filesystem watchers (when `search.watch` is on).
    pub watch: bool,
    /// fff-search's options from Settings.
    pub search: SearchSettings,
}

#[derive(Clone)]
struct RootIndex {
    path: PathBuf,
    /// `path` with symlinks resolved, to recognize folders spelled another
    /// way (`/private/var/…` for `/var/…` on macOS).
    canonical: PathBuf,
    picker: SharedFilePicker,
}

impl RootIndex {
    fn start(
        path: &Path,
        frecency: &SharedFrecency,
        watch: bool,
        settings: &SearchSettings,
    ) -> anyhow::Result<RootIndex> {
        let picker = SharedFilePicker::default();
        FilePicker::new_with_shared_state(
            picker.clone(),
            frecency.clone(),
            FilePickerOptions {
                base_path: path.to_string_lossy().into_owned(),
                mode: FFFMode::Neovim,
                watch: watch && settings.watch,
                follow_symlinks: settings.follow_symlinks,
                enable_content_indexing: settings.content_index,
                enable_mmap_cache: settings.preload_files,
                git_recency: GitRecencyConfig {
                    enabled: settings.git_recency,
                    ..Default::default()
                },
                // Roots are chosen by the user, so home and `/` are allowed.
                enable_fs_root_scanning: true,
                enable_home_dir_scanning: true,
                ..Default::default()
            },
        )
        .map_err(|e| anyhow::anyhow!("cannot index {}: {e}", path.display()))?;
        Ok(RootIndex {
            path: path.to_path_buf(),
            canonical: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
            picker,
        })
    }

    /// `scope`, a folder in this root, spelled the way fff spells the root's
    /// base path, which starts every path fff returns. On Windows fff
    /// canonicalizes the base (long names, no `\\?\`), so it can differ from
    /// the root as given, as `C:\Users\RUNNER~1` does from `C:\Users\runneradmin`.
    fn in_base(&self, scope: &Path, base: &Path) -> PathBuf {
        match scope.strip_prefix(&self.path) {
            Ok(rest) if rest.as_os_str().is_empty() => base.to_path_buf(),
            Ok(rest) => base.join(rest),
            Err(_) => scope.to_path_buf(),
        }
    }

    /// Whether its index is still being built; cheaper than [`Self::status`],
    /// which counts folders.
    fn scanning(&self) -> bool {
        let guard = self.picker.read().ok();
        guard
            .as_ref()
            .and_then(|g| g.as_ref())
            .is_none_or(|p| p.get_scan_progress().is_scanning)
    }

    fn status(&self) -> RootStatus {
        let guard = self.picker.read().ok();
        let picker = guard.as_ref().and_then(|g| g.as_ref());
        match picker {
            Some(p) => {
                let progress = p.get_scan_progress();
                RootStatus {
                    path: self.path.clone(),
                    scanning: progress.is_scanning,
                    finishing: !progress.is_scanning && p.is_post_scan_active(),
                    indexed_files: if progress.is_scanning {
                        progress.scanned_files_count
                    } else {
                        p.live_file_count()
                    },
                    indexed_dirs: p.get_dirs().iter().filter(|d| !d.is_deleted()).count(),
                    git_repo: p.has_git_repo(),
                }
            }
            None => RootStatus {
                path: self.path.clone(),
                scanning: true,
                finishing: false,
                indexed_files: 0,
                indexed_dirs: 0,
                git_repo: false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootStatus {
    pub path: PathBuf,
    pub scanning: bool,
    /// Scanned and searchable, while fff-search still reads each file once
    /// to flag binaries, which takes minutes for a large folder.
    #[serde(default)]
    pub finishing: bool,
    /// Files scanned so far while scanning, or live files once ready.
    pub indexed_files: usize,
    #[serde(default)]
    pub indexed_dirs: usize,
    pub git_repo: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchRequest {
    pub query: String,
    /// Only return results under this folder.
    pub scope: Option<PathBuf>,
    pub kind: KindFilter,
    pub limit: usize,
    pub show_hidden: bool,
    /// With [`KindFilter::Any`], leave folders out.
    pub files_only: bool,
}

/// What the index knows about one folder's items: see [`Engine::folder_stats`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderStats {
    /// Bytes under each subfolder.
    pub sizes: std::collections::HashMap<PathBuf, u64>,
    /// Git status of each file that isn't clean: `modified`, `untracked`, …
    pub git: std::collections::HashMap<PathBuf, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub entry: FileEntry,
    pub score: i32,
    /// Byte ranges in `entry.name` to highlight.
    pub highlights: Vec<(usize, usize)>,
    pub git_status: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub total_matched: usize,
    pub total_indexed: usize,
    pub elapsed: Duration,
    /// True when some searched index was still scanning (results incomplete).
    pub indexing: bool,
}

/// How `GrepRequest::patterns` match.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrepModeKind {
    #[default]
    Plain,
    Regex,
    /// Tolerates typos and gaps.
    Fuzzy,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrepRequest {
    /// One or more patterns; a line matching any of them is a hit.
    pub patterns: Vec<String>,
    pub mode: GrepModeKind,
    pub scope: Option<PathBuf>,
    pub limit: usize,
    /// Context lines to include around each match.
    pub before: usize,
    pub after: usize,
    /// Matches kept per file; 0 keeps up to `limit`.
    pub per_file: usize,
    /// Strip leading whitespace from lines (for display).
    pub trim: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrepHit {
    pub path: PathBuf,
    pub line: u64,
    pub column: usize,
    /// Offset of the line from the start of the file.
    pub byte_offset: u64,
    pub text: String,
    /// Byte ranges within `text`.
    pub ranges: Vec<(usize, usize)>,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// An indexed file or folder, for listing the whole index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexedItem {
    pub path: PathBuf,
    /// The search location it belongs to.
    pub root: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GrepResponse {
    pub hits: Vec<GrepHit>,
    pub files_with_matches: usize,
    pub files_searched: usize,
    pub elapsed: Duration,
    pub indexing: bool,
    /// The time limit (Settings › Search engine) ended the search before
    /// every file was searched.
    pub stopped: bool,
}

/// A change subscription from [`Engine::watch`]; dropping it unsubscribes.
pub struct Watch {
    pub events: std::sync::mpsc::Receiver<Vec<WatchEvent>>,
    subscriptions: Vec<(SharedFilePicker, WatchId)>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        for (picker, id) in &self.subscriptions {
            picker.unwatch(*id);
        }
    }
}

#[derive(Default)]
struct Indexes {
    roots: Vec<RootIndex>,
    adhoc: Vec<RootIndex>,
}

/// The folders apps are read from, and what was read.
#[derive(Default)]
struct Apps {
    dirs: Vec<PathBuf>,
    /// Read by the first search that wants it, and again once a folder in it
    /// changes.
    list: Option<Arc<AppList>>,
}

pub struct Engine {
    indexes: RwLock<Indexes>,
    apps: RwLock<Apps>,
    /// Sizes of app bundles outside the roots, by path, with the bundle's
    /// modification time when it was added up.
    bundle_sizes: RwLock<HashMap<PathBuf, (Option<SystemTime>, u64)>>,
    frecency: SharedFrecency,
    queries: SharedQueryTracker,
    watch: bool,
    search: RwLock<SearchSettings>,
    generation: AtomicU64,
}

impl Engine {
    pub fn new(options: EngineOptions) -> Arc<Engine> {
        let (frecency, queries) = open_dbs(options.db_dir.as_deref());
        let engine = Arc::new(Engine {
            indexes: RwLock::new(Indexes::default()),
            apps: RwLock::new(Apps {
                dirs: options.app_dirs,
                list: None,
            }),
            bundle_sizes: RwLock::default(),
            frecency,
            queries,
            watch: options.watch,
            search: RwLock::new(options.search),
            generation: AtomicU64::new(0),
        });
        engine.set_roots(&options.roots);
        engine
    }

    /// Replace the indexed roots. Unchanged roots keep their index.
    pub fn set_roots(&self, roots: &[PathBuf]) {
        let roots = normalize_roots(roots);
        let settings = self.search_settings();
        let mut indexes = self.indexes.write().unwrap_or_else(|e| e.into_inner());
        let mut keep: Vec<RootIndex> = Vec::new();
        for root in &roots {
            if let Some(existing) = indexes.roots.iter().find(|r| &r.path == root) {
                keep.push(existing.clone());
                continue;
            }
            match RootIndex::start(root, &self.frecency, self.watch, &settings) {
                Ok(index) => keep.push(index),
                Err(err) => log::warn!("{err}"),
            }
        }
        indexes.roots = keep;
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Read apps from `dirs` from now on; empty stops finding apps.
    pub fn set_app_dirs(&self, dirs: &[PathBuf]) {
        *self.apps.write().unwrap_or_else(|e| e.into_inner()) = Apps {
            dirs: dirs.to_vec(),
            list: None,
        };
    }

    /// The app list, read again first if a folder in it changed.
    fn app_list(&self) -> Arc<AppList> {
        {
            let apps = self.apps.read().unwrap_or_else(|e| e.into_inner());
            if let Some(list) = apps.list.as_ref().filter(|l| l.is_current()) {
                return Arc::clone(list);
            }
        }
        let mut apps = self.apps.write().unwrap_or_else(|e| e.into_inner());
        let list = Arc::new(AppList::scan(&apps.dirs));
        apps.list = Some(Arc::clone(&list));
        list
    }

    pub fn search_settings(&self) -> SearchSettings {
        self.search
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Use new fff-search options. Changing how folders are indexed (symlinks,
    /// watching, git recency, content index, preloading) rebuilds every index.
    pub fn set_search_settings(&self, settings: SearchSettings) {
        let reindex = {
            let mut current = self.search.write().unwrap_or_else(|e| e.into_inner());
            let reindex = current.index_options() != settings.index_options();
            *current = settings;
            reindex
        };
        if reindex {
            let roots = self.roots();
            *self.indexes.write().unwrap_or_else(|e| e.into_inner()) = Indexes::default();
            self.set_roots(&roots);
        }
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.read_indexes()
            .roots
            .iter()
            .map(|r| r.path.clone())
            .collect()
    }

    /// Bumped whenever the set of roots changes.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> Vec<RootStatus> {
        self.read_indexes()
            .roots
            .iter()
            .map(RootIndex::status)
            .collect()
    }

    pub fn is_scanning(&self) -> bool {
        self.roots_indexing() > 0
    }

    /// How many roots are still building their index. A search made while
    /// one is can run again as soon as this drops.
    pub fn roots_indexing(&self) -> usize {
        self.read_indexes()
            .roots
            .iter()
            .filter(|root| root.scanning())
            .count()
    }

    /// Block until every root finished its initial scan, or `timeout` passes.
    pub fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let pickers: Vec<SharedFilePicker> = self
            .read_indexes()
            .roots
            .iter()
            .map(|r| r.picker.clone())
            .collect();
        pickers.iter().all(|p| {
            let left = deadline.saturating_duration_since(Instant::now());
            p.wait_for_scan(left)
        })
    }

    /// Create the index covering `scope` if needed and wait up to `timeout`
    /// for its scan. Returns whether it is ready.
    pub fn prepare(&self, scope: Option<&Path>, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        self.indexes_for(scope).iter().all(|r| {
            let left = deadline.saturating_duration_since(Instant::now());
            r.picker.wait_for_scan(left)
        })
    }

    /// Rescan every root (for example after permissions changed).
    pub fn rescan(&self) {
        for root in &self.read_indexes().roots {
            if let Err(err) = root.picker.trigger_full_rescan_async(&self.frecency) {
                log::warn!("rescan {}: {err}", root.path.display());
            }
        }
    }

    /// `scope` spelled the way a root is, when it is inside one only after
    /// resolving symlinks; `None` when it needs no change.
    fn root_spelling(&self, scope: &Path) -> Option<PathBuf> {
        let indexes = self.read_indexes();
        if indexes.roots.iter().any(|r| scope.starts_with(&r.path)) {
            return None;
        }
        let real = std::fs::canonicalize(scope).ok()?;
        indexes.roots.iter().find_map(|r| {
            let rest = real.strip_prefix(&r.canonical).ok()?;
            Some(if rest.as_os_str().is_empty() {
                r.path.clone()
            } else {
                r.path.join(rest)
            })
        })
    }

    /// Indexes covering `scope`: the roots containing it, or a temporary
    /// index of the folder itself when no root does.
    fn indexes_for(&self, scope: Option<&Path>) -> Vec<RootIndex> {
        let Some(scope) = scope else {
            return self.read_indexes().roots.clone();
        };
        {
            let indexes = self.read_indexes();
            let covering: Vec<RootIndex> = indexes
                .roots
                .iter()
                .filter(|r| scope.starts_with(&r.path))
                .cloned()
                .collect();
            if !covering.is_empty() {
                return covering;
            }
            if let Some(index) = indexes.adhoc.iter().find(|r| r.path == scope) {
                return vec![index.clone()];
            }
        }
        match RootIndex::start(scope, &self.frecency, self.watch, &self.search_settings()) {
            Ok(index) => {
                let mut indexes = self.indexes.write().unwrap_or_else(|e| e.into_inner());
                indexes.adhoc.retain(|r| r.path != scope);
                indexes.adhoc.insert(0, index.clone());
                indexes.adhoc.truncate(MAX_ADHOC);
                vec![index]
            }
            Err(err) => {
                log::warn!("{err}");
                Vec::new()
            }
        }
    }

    pub fn search(&self, req: &SearchRequest) -> SearchResponse {
        if let Some(scope) = req.scope.as_deref().and_then(|s| self.root_spelling(s)) {
            return self.search(&SearchRequest {
                scope: Some(scope),
                ..req.clone()
            });
        }
        let started = Instant::now();
        let limit = if req.limit == 0 { 500 } else { req.limit };
        let indexes = self.indexes_for(req.scope.as_deref());
        let indexing = indexes.iter().any(|r| r.status().scanning);

        let parts: Vec<RootPart> = std::thread::scope(|s| {
            let handles: Vec<_> = indexes
                .iter()
                .map(|root| s.spawn(move || self.search_root(root, req, limit)))
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });

        let mut total_matched = 0;
        let mut total_indexed = 0;
        let mut raw = Vec::new();
        for part in parts {
            total_matched += part.total_matched;
            total_indexed += part.total_indexed;
            raw.extend(part.hits);
        }
        let words: Vec<String> = fuzzy_text(&req.query)
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        if wants_apps(req, &words) {
            let apps = app_hits(self.app_list().entries(), &words);
            total_matched += apps.len();
            raw.extend(apps);
        }
        // Typo-tolerant matches are a fallback: when some results contain every
        // word as an in-order subsequence (fzf-style), keep only those.
        let base = req.scope.as_deref();
        let strict = |hit: &RawHit| {
            let shown = base
                .and_then(|b| hit.path.strip_prefix(b).ok())
                .unwrap_or(&hit.path)
                .to_string_lossy()
                .to_lowercase();
            words.iter().all(|w| is_subsequence(w, &shown))
        };
        if !words.is_empty() && raw.iter().any(&strict) {
            raw.retain(strict);
        }
        for hit in &mut raw {
            hit.score = rerank(hit, &words);
        }
        raw.sort_by_key(|h| std::cmp::Reverse(h.score));
        let mut seen = HashSet::new();
        raw.retain(|h| seen.insert(h.path.clone()));
        raw.truncate(limit);

        let needle = fuzzy_text(&req.query);
        let hits = raw
            .into_iter()
            .filter_map(|h| h.into_hit(&needle, req.show_hidden))
            .collect();
        SearchResponse {
            hits,
            total_matched,
            total_indexed,
            elapsed: started.elapsed(),
            indexing,
        }
    }

    fn search_root(&self, root: &RootIndex, req: &SearchRequest, limit: usize) -> RootPart {
        let mut part = RootPart::default();
        let Ok(guard) = root.picker.read() else {
            return part;
        };
        let Some(picker) = guard.as_ref() else {
            return part;
        };
        let base = picker.base_path().to_path_buf();
        part.total_indexed = picker.live_file_count();
        let scope = req.scope.as_deref().map(|s| root.in_base(s, &base));

        let scope_rel = scope
            .as_deref()
            .filter(|s| *s != base.as_path())
            .and_then(|s| s.strip_prefix(&base).ok())
            .map(|rel| rel.to_string_lossy().replace('\\', "/"));
        // A glob narrows the search inside fff; the prefix check below is exact.
        let scope_glob = scope_rel
            .as_deref()
            .filter(|rel| {
                !rel.chars()
                    .any(|c| c.is_whitespace() || "*?[]{}!\\".contains(c))
            })
            .map(|rel| format!("{rel}/**"));

        let user = req.query.trim();
        let single_char =
            fuzzy_text(user).chars().count() == 1 && !user.contains(char::is_whitespace);
        let mut query = if single_char {
            String::new()
        } else {
            user.to_string()
        };
        for ext in req.kind.extensions() {
            query.push_str(" *.");
            query.push_str(ext);
        }
        if let Some(glob) = &scope_glob {
            query.push(' ');
            query.push_str(glob);
        }
        let mut query = query.trim().to_string();
        // In mixed search a query ending in `/` means "folders only"; keep that
        // for a lone folder token but not for "main src/"-style path filters.
        if req.kind == KindFilter::Any
            && query.ends_with('/')
            && let Some((head, last)) = query.rsplit_once(char::is_whitespace)
        {
            query = format!("{last} {head}");
        }

        // fff ignores one-character words, so over-fetch and filter by name.
        // Over-fetch so the re-rank and package filter have candidates to work with.
        let fetch = if single_char || (scope_rel.is_some() && scope_glob.is_none()) {
            limit.max(20_000)
        } else {
            (limit * 4).max(2_000)
        };
        let qt_guard = self.queries.read().ok();
        let qt = qt_guard.as_ref().and_then(|g| g.as_ref());
        let options = FuzzySearchOptions {
            max_threads: self.search_settings().threads,
            project_path: Some(&base),
            pagination: PaginationArgs {
                offset: 0,
                limit: fetch,
            },
            ..Default::default()
        };

        let in_scope = |path: &Path| {
            let scoped = match &scope {
                Some(scope) => path.starts_with(scope) && path != scope,
                None => true,
            };
            scoped && !inside_package(path, scope.as_deref())
        };
        let name_ok = |name: &str| {
            !single_char
                || name
                    .to_lowercase()
                    .contains(&fuzzy_text(user).to_lowercase())
        };

        if req.kind == KindFilter::Folder {
            let parsed = QueryParser::new(DirSearchConfig).parse(&query);
            let result = picker.fuzzy_search_directories(&parsed, options);
            part.total_matched = result.total_matched;
            for (item, score) in result.items.iter().zip(&result.scores) {
                let rel = item.relative_path(picker);
                if rel.is_empty() {
                    continue;
                }
                let path = join_rel(&base, &rel);
                let name = display_name(&path);
                if in_scope(&path) && name_ok(&name) {
                    part.hits.push(RawHit::dir(path, score.total));
                }
            }
        } else if req.kind == KindFilter::Any && !req.files_only {
            let parsed = QueryParser::new(MixedSearchConfig).parse(&query);
            let result = picker.fuzzy_search_mixed(&parsed, qt, options);
            part.total_matched = result.total_matched;
            for (item, score) in result.items.iter().zip(&result.scores) {
                let hit = match item {
                    MixedItemRef::File(f) => {
                        let path = join_rel(&base, &f.relative_path(picker));
                        RawHit::file(
                            path,
                            f.size,
                            f.modified,
                            fff_search::git::format_git_status_opt(f.git_status),
                            score.total,
                        )
                    }
                    MixedItemRef::Dir(d) => {
                        let rel = d.relative_path(picker);
                        if rel.is_empty() {
                            continue;
                        }
                        RawHit::dir(join_rel(&base, &rel), score.total)
                    }
                };
                if in_scope(&hit.path) && name_ok(&display_name(&hit.path)) {
                    part.hits.push(hit);
                }
            }
        } else {
            let parsed = QueryParser::default().parse(&query);
            let result = picker.fuzzy_search(&parsed, qt, options);
            part.total_matched = result.total_matched;
            for (item, score) in result.items.iter().zip(&result.scores) {
                let path = join_rel(&base, &item.relative_path(picker));
                let kind = kind::classify(&path, false);
                if in_scope(&path) && req.kind.matches(kind) && name_ok(&display_name(&path)) {
                    part.hits.push(RawHit::file(
                        path,
                        item.size,
                        item.modified,
                        fff_search::git::format_git_status_opt(item.git_status),
                        score.total,
                    ));
                }
            }
        }
        part
    }

    /// Absolute paths of indexed files matching `pattern` (a glob over the
    /// path inside each root, like `**/*.app/Contents/Info.plist`), across
    /// every root. Nothing is re-ranked or hidden, package contents included.
    pub fn glob(&self, pattern: &str, limit: usize) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for root in &self.read_indexes().roots {
            let Ok(guard) = root.picker.read() else {
                continue;
            };
            let Some(picker) = guard.as_ref() else {
                continue;
            };
            let base = picker.base_path().to_path_buf();
            let result = picker.glob(
                pattern,
                FuzzySearchOptions {
                    project_path: Some(&base),
                    pagination: PaginationArgs { offset: 0, limit },
                    ..Default::default()
                },
            );
            found.extend(
                result
                    .items
                    .iter()
                    .map(|item| join_rel(&base, &item.relative_path(picker))),
            );
        }
        found.sort();
        found.dedup();
        found.truncate(limit);
        found
    }

    /// Files modified in the last 30 days across the roots, newest first,
    /// skipping hidden paths, `Library` folders, and package contents.
    pub fn recents(&self, limit: usize) -> Vec<FileEntry> {
        const WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);
        let cutoff = SystemTime::now()
            .checked_sub(WINDOW)
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        let mut found: Vec<(u64, PathBuf, u64)> = Vec::new();
        for root in &self.read_indexes().roots {
            let Ok(guard) = root.picker.read() else {
                continue;
            };
            let Some(picker) = guard.as_ref() else {
                continue;
            };
            let base = picker.base_path().to_path_buf();
            let query = QueryParser::default().parse("");
            // An empty query ranks by frecency, then by newest modification.
            let result = picker.fuzzy_search(
                &query,
                None,
                FuzzySearchOptions {
                    pagination: PaginationArgs {
                        offset: 0,
                        limit: (limit * 4).max(4_000),
                    },
                    ..Default::default()
                },
            );
            for item in result.items {
                if item.modified < cutoff {
                    continue;
                }
                let rel = item.relative_path(picker);
                if rel
                    .split('/')
                    .any(|part| part.starts_with('.') || part == "Library")
                {
                    continue;
                }
                let path = join_rel(&base, &rel);
                if !inside_package(&path, None) {
                    found.push((item.modified, path, item.size));
                }
            }
        }
        found.sort_by_key(|f| std::cmp::Reverse(f.0));
        found.truncate(limit);
        found
            .into_iter()
            .filter_map(|(modified, path, size)| {
                RawHit::file(path, size, modified, None, 0)
                    .into_hit("", false)
                    .map(|hit| hit.entry)
            })
            .collect()
    }

    /// Content search across the indexes covering `scope`.
    pub fn grep(&self, req: &GrepRequest) -> GrepResponse {
        if let Some(scope) = req.scope.as_deref().and_then(|s| self.root_spelling(s)) {
            return self.grep(&GrepRequest {
                scope: Some(scope),
                ..req.clone()
            });
        }
        let settings = self.search_settings();
        let started = Instant::now();
        let limit = if req.limit == 0 { 200 } else { req.limit };
        let indexes = self.indexes_for(req.scope.as_deref());
        let mut response = GrepResponse {
            indexing: indexes.iter().any(|r| r.status().scanning),
            ..Default::default()
        };
        let mut patterns: Vec<String> = req
            .patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .cloned()
            .collect();
        if patterns.is_empty() {
            return response;
        }
        // fff matches several patterns as literal text; several regexes
        // mean "any of these", so join them into one.
        if req.mode == GrepModeKind::Regex && patterns.len() > 1 {
            let joined = patterns
                .iter()
                .map(|p| format!("(?:{p})"))
                .collect::<Vec<_>>()
                .join("|");
            patterns = vec![joined];
        }
        let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
        for root in &indexes {
            let Ok(guard) = root.picker.read() else {
                continue;
            };
            let Some(picker) = guard.as_ref() else {
                continue;
            };
            let base = picker.base_path().to_path_buf();
            let scope = req.scope.as_deref().map(|s| root.in_base(s, &base));
            // A scope inside this root narrows the search with a glob.
            let scope_glob = scope
                .as_deref()
                .filter(|s| *s != base.as_path())
                .and_then(|s| s.strip_prefix(&base).ok())
                .map(|rel| rel.to_string_lossy().replace('\\', "/"))
                .filter(|rel| !rel.contains(char::is_whitespace))
                .map(|rel| format!(" {rel}/**"));
            let options = GrepSearchOptions {
                mode: match req.mode {
                    GrepModeKind::Plain => GrepMode::PlainText,
                    GrepModeKind::Regex => GrepMode::Regex,
                    GrepModeKind::Fuzzy => GrepMode::Fuzzy,
                },
                page_limit: limit,
                max_matches_per_file: if req.per_file == 0 {
                    limit
                } else {
                    req.per_file
                },
                max_file_size: settings.max_file_size_mb.max(1) * 1024 * 1024,
                casing: Some(match settings.case {
                    SearchCase::Smart => Casing::Smart,
                    SearchCase::Sensitive => Casing::Sensitive,
                    SearchCase::Insensitive => Casing::Insensitive,
                }),
                time_budget_ms: settings.time_limit_secs * 1000,
                // fff-search otherwise waits for a first match before
                // counting, so a search that finds nothing ran to the end.
                enforce_time_budget: true,
                trim_whitespace: req.trim,
                before_context: req.before,
                after_context: req.after,
                ..Default::default()
            };
            let parser = QueryParser::new(GrepConfig);
            let result = if let [only] = patterns.as_slice() {
                let text = format!("{only}{}", scope_glob.as_deref().unwrap_or(""));
                picker.grep(&parser.parse(&text), &options)
            } else {
                // Each pattern may carry file constraints; the rest is text.
                let mut constraints = Vec::new();
                let mut texts: Vec<String> = Vec::new();
                let scoped: Vec<String> = patterns
                    .iter()
                    .enumerate()
                    .map(|(i, p)| match (&scope_glob, i) {
                        (Some(glob), 0) => format!("{p}{glob}"),
                        _ => p.to_string(),
                    })
                    .collect();
                for pattern in &scoped {
                    let query = parser.parse(pattern);
                    let text = query.grep_text();
                    constraints.extend(query.constraints);
                    if !text.is_empty() {
                        texts.push(text);
                    }
                }
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                picker.multi_grep(&refs, &constraints, &options)
            };
            let limit_reached = settings.time_limit_secs > 0
                && started.elapsed() >= Duration::from_secs(settings.time_limit_secs);
            response.stopped |= limit_reached && result.next_file_offset > 0;
            response.files_with_matches += result.files_with_matches;
            response.files_searched += result.total_files_searched;
            for m in &result.matches {
                let Some(file) = result.files.get(m.file_index) else {
                    continue;
                };
                let path = join_rel(&base, &file.relative_path(picker));
                if let Some(scope) = &scope
                    && !path.starts_with(scope)
                {
                    continue;
                }
                response.hits.push(GrepHit {
                    path,
                    line: m.line_number,
                    column: m.col,
                    byte_offset: m.byte_offset,
                    text: m.line_content.clone(),
                    ranges: m
                        .match_byte_offsets
                        .iter()
                        .map(|&(a, b)| (a as usize, b as usize))
                        .collect(),
                    context_before: m.context_before.clone(),
                    context_after: m.context_after.clone(),
                });
                if response.hits.len() >= limit {
                    break;
                }
            }
            if response.hits.len() >= limit {
                break;
            }
        }
        response.elapsed = started.elapsed();
        response
    }

    /// Every indexed file, or folder, across the roots; with a query, the
    /// fuzzy matches in fff's own order. At most `limit` (0 for no limit).
    pub fn list(&self, dirs: bool, query: Option<&str>, limit: usize) -> Vec<IndexedItem> {
        let limit = if limit == 0 { usize::MAX } else { limit };
        let threads = self.search_settings().threads;
        let mut items = Vec::new();
        for root in &self.read_indexes().roots {
            if items.len() >= limit {
                break;
            }
            let Ok(guard) = root.picker.read() else {
                continue;
            };
            let Some(picker) = guard.as_ref() else {
                continue;
            };
            let base = picker.base_path().to_path_buf();
            let left = limit - items.len();
            let item = |rel: String, is_dir: bool, size: u64, modified: u64| IndexedItem {
                path: join_rel(&base, &rel),
                root: base.clone(),
                is_dir,
                size,
                modified,
            };
            match (dirs, query.map(str::trim).filter(|q| !q.is_empty())) {
                // The overflow files (added since the scan) are the end of
                // `get_files`, so they're already in it.
                (false, None) => items.extend(
                    picker
                        .get_files()
                        .iter()
                        .filter(|f| !f.is_deleted())
                        .take(left)
                        .map(|f| item(f.relative_path(picker), false, f.size, f.modified)),
                ),
                (true, None) => items.extend(
                    picker
                        .get_dirs()
                        .iter()
                        .filter(|d| !d.is_deleted())
                        .map(|d| d.relative_path(picker))
                        .filter(|rel| !rel.is_empty())
                        .take(left)
                        .map(|rel| item(rel, true, 0, 0)),
                ),
                (false, Some(q)) => {
                    let parsed = QueryParser::default().parse(q);
                    let options = FuzzySearchOptions {
                        max_threads: threads,
                        project_path: Some(&base),
                        pagination: PaginationArgs {
                            offset: 0,
                            limit: left.min(100_000),
                        },
                        ..Default::default()
                    };
                    let result = picker.fuzzy_search(&parsed, None, options);
                    items.extend(
                        result
                            .items
                            .iter()
                            .map(|f| item(f.relative_path(picker), false, f.size, f.modified)),
                    );
                }
                (true, Some(q)) => {
                    let parsed = QueryParser::new(DirSearchConfig).parse(q);
                    let options = FuzzySearchOptions {
                        max_threads: threads,
                        project_path: Some(&base),
                        pagination: PaginationArgs {
                            offset: 0,
                            limit: left.min(100_000),
                        },
                        ..Default::default()
                    };
                    let result = picker.fuzzy_search_directories(&parsed, options);
                    items.extend(
                        result
                            .items
                            .iter()
                            .map(|d| d.relative_path(picker))
                            .filter(|rel| !rel.is_empty())
                            .map(|rel| item(rel, true, 0, 0)),
                    );
                }
            }
        }
        if dirs && items.len() < limit {
            let words: Vec<String> = query
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            let roots = self.roots();
            let list = self.app_list();
            let apps = list
                .entries()
                .iter()
                .filter(|p| !roots.iter().any(|r| p.starts_with(r)))
                .filter(|p| name_holds(p, &words))
                .map(|p| IndexedItem {
                    path: p.clone(),
                    root: list.dir_of(p).unwrap_or(p).to_path_buf(),
                    is_dir: true,
                    size: 0,
                    modified: 0,
                });
            items.extend(apps.take(limit - items.len()));
        }
        items.truncate(limit);
        items
    }

    /// Sizes of the app bundles directly in `dir`, an app folder no root
    /// covers, which the index can't add up, so the app folders show app
    /// sizes as Finder does. The bundles come from the app list, so no other
    /// folder is read. Each is walked once, bundles in parallel, and added up
    /// again only when its modification time changes.
    ///
    /// # Returns
    ///
    /// Bytes in each bundle, by path; empty for any other folder
    pub fn bundle_sizes(&self, dir: &Path) -> HashMap<PathBuf, u64> {
        if self.roots().iter().any(|r| dir.starts_with(r)) {
            return HashMap::new();
        }
        let list = self.app_list();
        let bundles: Vec<(PathBuf, Option<SystemTime>)> = list
            .apps()
            .filter(|p| p.parent() == Some(dir))
            .map(|p| {
                let time = std::fs::metadata(p).and_then(|m| m.modified()).ok();
                (p.clone(), time)
            })
            .collect();
        let mut sizes = HashMap::with_capacity(bundles.len());
        let mut todo = Vec::new();
        {
            let known = self.bundle_sizes.read().unwrap_or_else(|e| e.into_inner());
            for (path, time) in bundles {
                match known.get(&path) {
                    Some((then, bytes)) if *then == time => {
                        sizes.insert(path, *bytes);
                    }
                    _ => todo.push((path, time)),
                }
            }
        }
        let never = AtomicBool::new(false);
        let added: Vec<(PathBuf, Option<SystemTime>, u64)> = todo
            .into_par_iter()
            .map(|(path, time)| {
                let bytes = crate::info::folder_size(&path, &never).bytes;
                (path, time, bytes)
            })
            .collect();
        let mut known = self.bundle_sizes.write().unwrap_or_else(|e| e.into_inner());
        for (path, time, bytes) in added {
            sizes.insert(path.clone(), bytes);
            known.insert(path, (time, bytes));
        }
        sizes
    }

    /// Folder sizes and git statuses for the items directly in `dir`, read
    /// from the index without touching the disk: each subfolder's total is
    /// every indexed file under it, and each file's git status is what the
    /// index last saw. Folders and files the index skips aren't included.
    pub fn folder_stats(&self, dir: &Path) -> FolderStats {
        self.folder_stats_for(&[dir.to_path_buf()])
            .pop()
            .unwrap_or_default()
    }

    /// [`Self::folder_stats`] for several folders at once, in the order
    /// given, with one pass over each root's files for all of them.
    pub fn folder_stats_for(&self, dirs: &[PathBuf]) -> Vec<FolderStats> {
        let mut all = vec![FolderStats::default(); dirs.len()];
        let indexes = self.read_indexes();
        // `set_roots` leaves out roots inside others, so no file counts twice.
        for root in &indexes.roots {
            let Ok(guard) = root.picker.read() else {
                continue;
            };
            let Some(picker) = guard.as_ref() else {
                continue;
            };
            // The overflow files (added since the scan) are the end of
            // `get_files`, so they're already in it.
            let files = || picker.get_files().iter().filter(|f| !f.is_deleted());
            let base = picker.base_path();
            let mut root_total: Option<u64> = None;
            // Each folder in this root, as the start of fff-search's relative
            // paths: `/` between names on every system, and one at the end.
            let mut wanted: Vec<(usize, String)> = Vec::new();
            for (ix, dir) in dirs.iter().enumerate() {
                // A root inside `dir` adds up as one of its subfolders.
                if let Ok(below) = root.path.strip_prefix(dir)
                    && let Some(first) = below.components().next()
                {
                    let total = *root_total.get_or_insert_with(|| files().map(|f| f.size).sum());
                    *all[ix].sizes.entry(dir.join(first)).or_default() += total;
                    continue;
                }
                if let Ok(rel) = root.in_base(dir, base).strip_prefix(base) {
                    let mut prefix = rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    if !prefix.is_empty() {
                        prefix.push('/');
                    }
                    wanted.push((ix, prefix));
                }
            }
            if wanted.is_empty() {
                continue;
            }
            // A file can only be under a folder whose path is no longer than
            // its own folder's, which is known without reading the path.
            let shortest = wanted.iter().map(|(_, p)| p.len()).min().unwrap_or(0);
            for file in files() {
                if file.filename_offset_in_relative_path() < shortest {
                    continue;
                }
                let path = file.relative_path(picker);
                for (ix, prefix) in &wanted {
                    let Some(rest) = path.strip_prefix(prefix.as_str()) else {
                        continue;
                    };
                    let (dir, stats) = (&dirs[*ix], &mut all[*ix]);
                    match rest.split_once('/') {
                        Some((child, _)) => {
                            *stats.sizes.entry(dir.join(child)).or_default() += file.size;
                        }
                        None => {
                            if let Some(status) =
                                fff_search::git::format_git_status_opt(file.git_status)
                                    .filter(|s| *s != "clean" && *s != "unknown")
                            {
                                stats.git.insert(dir.join(rest), status.to_string());
                            }
                        }
                    }
                }
            }
        }
        all
    }

    /// Subscribe to changes under the roots. `pattern` is a glob relative to
    /// each root, an absolute path, or empty for everything. Events arrive in
    /// debounced batches until the returned [`Watch`] is dropped.
    pub fn watch(&self, pattern: &str, ignore: &[String]) -> anyhow::Result<Watch> {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut subscriptions = Vec::new();
        let mut last_error = None;
        let pattern = pattern.trim();
        for root in &self.read_indexes().roots {
            // An absolute pattern only applies to the root that holds it.
            if Path::new(pattern).is_absolute() && !Path::new(pattern).starts_with(&root.path) {
                continue;
            }
            let tx = tx.clone();
            let options = WatchOptions {
                ignore: ignore.to_vec(),
            };
            match root.picker.watch(pattern, options, move |_, events| {
                let _ = tx.send(events.to_vec());
            }) {
                Ok(id) => subscriptions.push((root.picker.clone(), id)),
                Err(err) => last_error = Some(err),
            }
        }
        if subscriptions.is_empty() {
            return Err(match last_error {
                Some(err) => anyhow::anyhow!("cannot watch {pattern:?}: {err}"),
                None => anyhow::anyhow!("{pattern:?} is outside every search location"),
            });
        }
        Ok(Watch {
            events: rx,
            subscriptions,
        })
    }

    /// Record that the user opened `path`, boosting it in later searches.
    pub fn track_open(&self, path: &Path, query: Option<&str>) {
        let Ok(frecency) = self.frecency.read() else {
            return;
        };
        let Some(tracker) = frecency.as_ref() else {
            return;
        };
        if let Err(err) = tracker.track_access(path) {
            log::debug!("track_access {}: {err}", path.display());
            return;
        }
        let roots = self.read_indexes().roots.clone();
        for root in roots.iter().filter(|r| path.starts_with(&r.path)) {
            if let Ok(mut guard) = root.picker.write()
                && let Some(picker) = guard.as_mut()
            {
                let _ = picker.update_single_file_frecency(path, tracker);
            }
            if let Some(query) = query.filter(|q| !q.trim().is_empty())
                && let Ok(mut qt) = self.queries.write()
                && let Some(qt) = qt.as_mut()
            {
                let _ = qt.track_query_completion(query.trim(), &root.path, path);
            }
        }
    }

    fn read_indexes(&self) -> std::sync::RwLockReadGuard<'_, Indexes> {
        self.indexes.read().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let indexes = self.indexes.get_mut().unwrap_or_else(|e| e.into_inner());
        for root in indexes.roots.iter().chain(indexes.adhoc.iter()) {
            root.picker.cancel();
        }
    }
}

#[derive(Default)]
struct RootPart {
    hits: Vec<RawHit>,
    total_matched: usize,
    total_indexed: usize,
}

struct RawHit {
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: u64,
    git_status: Option<&'static str>,
    score: i32,
}

impl RawHit {
    fn file(
        path: PathBuf,
        size: u64,
        modified: u64,
        git_status: Option<&'static str>,
        score: i32,
    ) -> RawHit {
        RawHit {
            path,
            is_dir: false,
            size,
            modified,
            git_status,
            score,
        }
    }

    fn dir(path: PathBuf, score: i32) -> RawHit {
        RawHit {
            path,
            is_dir: true,
            size: 0,
            modified: 0,
            git_status: None,
            score,
        }
    }

    fn into_hit(self, needle: &str, show_hidden: bool) -> Option<SearchHit> {
        let entry = if self.is_dir {
            // Directory rows carry no metadata in the index.
            FileEntry::from_path(&self.path)?
        } else {
            let meta_modified = (self.modified > 0)
                .then(|| SystemTime::UNIX_EPOCH + Duration::from_secs(self.modified));
            let name = display_name(&self.path);
            FileEntry {
                kind: kind::classify(&self.path, false),
                hidden: name.starts_with('.'),
                name,
                is_dir: false,
                size: Some(self.size),
                modified: meta_modified,
                is_symlink: false,
                created: None,
                mode: None,
                last_opened: None,
                readonly: false,
                path: self.path,
            }
        };
        if entry.hidden && !show_hidden {
            return None;
        }
        Some(SearchHit {
            highlights: highlight(&entry.name, needle),
            entry,
            score: self.score,
            git_status: self
                .git_status
                .filter(|s| *s != "clean" && *s != "unknown")
                .map(str::to_owned),
        })
    }
}

/// Finder treats bundles (`.app`, `.framework`, …) as single items, so their
/// contents are hidden unless the search is scoped inside one.
fn inside_package(path: &Path, scope: Option<&Path>) -> bool {
    const PACKAGES: [&str; 7] = [
        ".app",
        ".framework",
        ".bundle",
        ".xcodeproj",
        ".xcworkspace",
        ".photoslibrary",
        ".plugin",
    ];
    let Some(parent) = path.parent() else {
        return false;
    };
    parent.ancestors().any(|dir| {
        if scope.is_some_and(|s| s.starts_with(dir)) {
            return false;
        }
        dir.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| PACKAGES.iter().any(|ext| n.ends_with(ext)))
    })
}

/// The apps and app folders in `entries` (from an [`AppList`]) whose names
/// hold every one of a query's fuzzy `words` in order. They're scored about
/// as fff-search scores a folder by its name: 15 points a letter for a word
/// found whole and 10 for one spread out, and none for a one-letter query,
/// which fff-search doesn't score either. [`rerank`] adds the name bonuses.
/// An app is one item wherever its folder is, as in Finder, so it's spared
/// rerank's depth penalty; otherwise `/System/Applications/Utilities/
/// Terminal.app` would lose to a file in home with the same name.
fn app_hits(entries: &[PathBuf], words: &[String]) -> Vec<RawHit> {
    let one_letter = matches!(words, [word] if word.chars().count() == 1);
    entries
        .iter()
        .filter_map(|path| {
            let name = display_name(path).to_lowercase();
            if !words.iter().all(|w| is_subsequence(w, &name)) {
                return None;
            }
            let matched: i32 = if one_letter {
                0
            } else {
                words
                    .iter()
                    .map(|w| {
                        let per_letter = if name.contains(w.as_str()) { 15 } else { 10 };
                        w.chars().count() as i32 * per_letter
                    })
                    .sum()
            };
            let depth = path.components().count() as i32;
            Some(RawHit::dir(path.clone(), matched + depth * DEPTH_PENALTY))
        })
        .collect()
}

/// Whether a search can find apps: one of every location, for folders,
/// with `words` (its fuzzy words) and nothing else.
fn wants_apps(req: &SearchRequest, words: &[String]) -> bool {
    let kind = match req.kind {
        KindFilter::Any => !req.files_only,
        KindFilter::Folder => true,
        KindFilter::Document
        | KindFilter::Image
        | KindFilter::Movie
        | KindFilter::Audio
        | KindFilter::Code
        | KindFilter::Archive => false,
    };
    // Constraints such as `*.rs` or `!test` can't be checked on an app.
    let constrained = req.query.split_whitespace().count() != words.len();
    req.scope.is_none() && kind && !constrained && !words.is_empty()
}

/// Whether `path`'s name holds each of `words` (lowercase) in order.
fn name_holds(path: &Path, words: &[String]) -> bool {
    let name = display_name(path).to_lowercase();
    words.iter().all(|w| is_subsequence(w, &name))
}

fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut chars = haystack.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

/// What [`rerank`] takes off for each folder in a hit's path.
const DEPTH_PENALTY: i32 = 3;

/// Finder-style ranking on top of fff's score: words found in the file
/// name beat words found only in its folders, and shallower paths win ties.
fn rerank(hit: &RawHit, words: &[String]) -> i32 {
    if words.is_empty() {
        return hit.score;
    }
    let name = display_name(&hit.path).to_lowercase();
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    let mut bonus = 0;
    let mut in_name = 0;
    for word in words {
        if stem == word || name == *word {
            bonus += 120;
        } else if name.starts_with(word.as_str()) {
            bonus += 70;
        } else if let Some(pos) = name.find(word.as_str()) {
            let boundary = name[..pos]
                .chars()
                .last()
                .is_some_and(|c| !c.is_alphanumeric());
            bonus += if boundary { 55 } else { 40 };
        } else {
            continue;
        }
        in_name += 1;
    }
    if in_name == words.len() {
        bonus += 40;
    }
    let depth = hit.path.components().count() as i32;
    hit.score + bonus - depth * DEPTH_PENALTY
}

/// fff returns `/`-separated relative paths on every OS.
fn join_rel(base: &Path, rel: &str) -> PathBuf {
    let rel = rel.trim_end_matches('/');
    let mut path = base.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        path.push(part);
    }
    path
}

/// The fuzzy words of a query, without constraint tokens.
fn fuzzy_text(query: &str) -> String {
    query
        .split_whitespace()
        .filter(|t| {
            !t.starts_with('!')
                && !t.contains(['*', '?', '[', '{'])
                && !t.contains(':')
                && !t.starts_with('/')
                && !t.ends_with('/')
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Greedy case-insensitive subsequence match of `needle` in `name`, for highlighting.
pub fn highlight(name: &str, needle: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let lower = name.to_lowercase();
    // Prefer a contiguous substring match when there is one.
    let joined: String = needle.iter().collect();
    if lower.len() == name.len()
        && let Some(start) = lower.find(&joined)
    {
        return vec![(start, start + joined.len())];
    }
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut want = needle.iter().peekable();
    for (i, c) in name.char_indices() {
        let Some(&&w) = want.peek() else { break };
        if c.to_lowercase().eq(std::iter::once(w)) {
            let end = i + c.len_utf8();
            match ranges.last_mut() {
                Some(last) if last.1 == i => last.1 = end,
                _ => ranges.push((i, end)),
            }
            want.next();
        }
    }
    if want.peek().is_some() {
        // Typo-tolerant matches may not be subsequences; show no partial noise.
        return Vec::new();
    }
    ranges
}

fn normalize_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in roots {
        let root = strip_trailing_separator(root);
        if !root.is_dir() {
            continue;
        }
        if out.iter().any(|r| root.starts_with(r)) {
            continue;
        }
        out.retain(|r| !r.starts_with(&root));
        out.push(root);
    }
    out
}

fn strip_trailing_separator(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let trimmed = s.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || (trimmed.len() == 2 && trimmed.ends_with(':')) {
        path.to_path_buf()
    } else {
        PathBuf::from(trimmed)
    }
}

fn open_dbs(dir: Option<&Path>) -> (SharedFrecency, SharedQueryTracker) {
    let frecency = SharedFrecency::default();
    let queries = SharedQueryTracker::default();
    let Some(dir) = dir else {
        return (frecency, queries);
    };
    match FrecencyTracker::open(dir.join("frecency")) {
        Ok(tracker) => {
            if let Err(err) = frecency.init(tracker) {
                log::warn!("frecency db: {err}");
            }
        }
        Err(err) => log::warn!("frecency db: {err}"),
    }
    match QueryTracker::open(dir.join("queries")) {
        Ok(tracker) => {
            if let Err(err) = queries.init(tracker) {
                log::warn!("query db: {err}");
            }
        }
        Err(err) => log::warn!("query db: {err}"),
    }
    (frecency, queries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_follows_the_base_spelling() {
        // As on Windows, where fff spells `C:\Users\RUNNER~1` in full.
        let root = RootIndex {
            path: "/short/tree".into(),
            canonical: "/long/tree".into(),
            picker: SharedFilePicker::default(),
        };
        let base = Path::new("/long/tree");
        let scope = |s: &str| root.in_base(Path::new(s), base);
        assert_eq!(scope("/short/tree"), base);
        assert_eq!(
            scope("/short/tree/Projects/atlas"),
            base.join("Projects/atlas")
        );
        assert_eq!(scope("/elsewhere"), Path::new("/elsewhere"));
    }

    #[test]
    fn highlights() {
        assert_eq!(highlight("Project Atlas.txt", "proj"), vec![(0, 4)]);
        assert_eq!(highlight("main.rs", "mrs"), vec![(0, 1), (5, 7)]);
        assert!(highlight("abc", "xyz").is_empty());
    }

    #[test]
    fn roots_are_normalized() {
        let tmp = std::env::temp_dir();
        let nested = tmp.join(format!("fff-norm-{}", std::process::id()));
        std::fs::create_dir_all(&nested).unwrap();
        let roots = normalize_roots(&[nested.clone(), tmp.clone(), PathBuf::from("/nope/x")]);
        assert_eq!(roots, vec![strip_trailing_separator(&tmp)]);
        std::fs::remove_dir_all(nested).unwrap();
    }

    #[test]
    fn subsequence() {
        assert!(is_subsequence("mrs", "main.rs"));
        assert!(!is_subsequence("engine", "crates/nu_plugin_fff/cargo.toml"));
    }

    #[test]
    fn glob_finds_files_inside_bundles() {
        let dir = std::env::temp_dir().join(format!("fff-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for app in ["One.app", "nested/Two.app"] {
            std::fs::create_dir_all(dir.join(app).join("Contents")).unwrap();
            std::fs::write(dir.join(app).join("Contents/Info.plist"), "<plist/>").unwrap();
        }
        std::fs::write(dir.join("Info.plist"), "<plist/>").unwrap();
        let engine = Engine::new(EngineOptions {
            roots: vec![dir.clone()],
            app_dirs: Vec::new(),
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(20)));
        let found = engine.glob("**/*.app/Contents/Info.plist", 100);
        // fff spells the root its own way on Windows (long names, `\`), so
        // compare the files rather than the spelling.
        let real = |p: &Path| std::fs::canonicalize(p).unwrap();
        assert_eq!(
            found.iter().map(|p| real(p)).collect::<Vec<_>>(),
            vec![
                real(&dir.join("One.app/Contents/Info.plist")),
                real(&dir.join("nested/Two.app/Contents/Info.plist")),
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn package_contents_are_hidden() {
        let app = Path::new("/Applications/Xcode.app/Contents/README");
        assert!(inside_package(app, None));
        assert!(!inside_package(Path::new("/Applications/Xcode.app"), None));
        assert!(!inside_package(
            app,
            Some(Path::new("/Applications/Xcode.app"))
        ));
    }

    #[test]
    fn rerank_prefers_name_matches() {
        let words = vec!["project".to_string(), "atlas".to_string()];
        let deep = RawHit::file("/a/b/c/d/e/projected.h".into(), 1, 1, None, 78);
        let good = RawHit::file("/a/Project Atlas.txt".into(), 1, 1, None, 70);
        assert!(rerank(&good, &words) > rerank(&deep, &words));
    }

    #[test]
    fn fuzzy_text_drops_constraints() {
        assert_eq!(fuzzy_text("main *.rs /src/ !test git:m"), "main");
    }

    #[test]
    fn adds_up_folders_from_the_index() {
        let dir = std::env::temp_dir().join(format!("fff-engine-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/deep")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("a/one.txt"), "12345").unwrap();
        std::fs::write(dir.join("a/deep/two.txt"), "123").unwrap();
        std::fs::write(dir.join("b/three.txt"), "1").unwrap();
        std::fs::write(dir.join("top.txt"), "xx").unwrap();
        let engine = Engine::new(EngineOptions {
            roots: vec![dir.clone()],
            app_dirs: Vec::new(),
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(10)));
        let stats = engine.folder_stats(&dir);
        assert_eq!(stats.sizes.get(&dir.join("a")), Some(&8));
        assert_eq!(stats.sizes.get(&dir.join("b")), Some(&1));
        assert_eq!(stats.sizes.len(), 2, "files at the top aren't folders");
        let above = engine.folder_stats(dir.parent().unwrap());
        assert_eq!(
            above.sizes.get(&dir),
            Some(&11),
            "a root inside counts whole"
        );
        let both = engine.folder_stats_for(&[dir.join("a"), dir.clone()]);
        assert_eq!(both[0].sizes.get(&dir.join("a/deep")), Some(&3));
        assert_eq!(
            both[1], stats,
            "one pass gives what one folder at a time does"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_root_inside_another_counts_once() {
        let dir = std::env::temp_dir().join(format!("fff-engine-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("inner/deep")).unwrap();
        std::fs::write(dir.join("inner/deep/one.txt"), "12345").unwrap();
        std::fs::write(dir.join("inner/two.txt"), "123").unwrap();
        let engine = Engine::new(EngineOptions {
            roots: vec![dir.clone(), dir.join("inner")],
            app_dirs: Vec::new(),
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(10)));
        assert_eq!(
            engine.folder_stats(&dir).sizes.get(&dir.join("inner")),
            Some(&8)
        );
        assert_eq!(
            engine
                .folder_stats(&dir.join("inner"))
                .sizes
                .get(&dir.join("inner/deep")),
            Some(&5)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn searches_a_folder() {
        let dir = std::env::temp_dir().join(format!("fff-engine-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("docs/notes")).unwrap();
        std::fs::write(dir.join("docs/notes/Project Atlas.txt"), "atlas").unwrap();
        std::fs::write(dir.join("docs/readme.md"), "hello").unwrap();
        std::fs::write(dir.join("photo.png"), [0u8; 4]).unwrap();
        let engine = Engine::new(EngineOptions {
            roots: vec![dir.clone()],
            app_dirs: Vec::new(),
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(10)));
        assert!(!engine.is_scanning());
        assert_eq!(engine.roots_indexing(), 0);

        let res = engine.search(&SearchRequest {
            query: "atlas".into(),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(res.hits[0].entry.name, "Project Atlas.txt");

        let res = engine.search(&SearchRequest {
            query: "notes".into(),
            kind: KindFilter::Folder,
            limit: 10,
            ..Default::default()
        });
        assert!(
            res.hits
                .iter()
                .any(|h| h.entry.is_dir && h.entry.name == "notes")
        );

        let res = engine.search(&SearchRequest {
            query: String::new(),
            kind: KindFilter::Image,
            limit: 10,
            ..Default::default()
        });
        assert_eq!(res.hits.len(), 1);
        assert_eq!(res.hits[0].entry.name, "photo.png");

        let res = engine.search(&SearchRequest {
            query: "readme".into(),
            scope: Some(dir.join("docs")),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(res.hits.len(), 1);

        let res = engine.grep(&GrepRequest {
            patterns: vec!["hello".into()],
            limit: 10,
            ..Default::default()
        });
        assert_eq!(res.hits.len(), 1);
        assert_eq!(res.hits[0].line, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_apps_without_indexing_them() {
        let dir = std::env::temp_dir().join(format!("fff-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("Users/me");
        let apps = dir.join("Applications");
        for app in ["Safari.app/Contents", "Notes.app", "Utilities/Terminal.app"] {
            std::fs::create_dir_all(apps.join(app)).unwrap();
        }
        std::fs::write(apps.join("Safari.app/Contents/Info.plist"), "plist").unwrap();
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        for file in ["terminal-notes.txt", "Documents/safari-bookmarks.html"] {
            std::fs::write(home.join(file), "x").unwrap();
        }
        let engine = Engine::new(EngineOptions {
            roots: vec![home.clone()],
            app_dirs: vec![apps.clone()],
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(10)));
        let find = |query: &str, kind: KindFilter, files_only: bool| -> Vec<String> {
            let req = SearchRequest {
                query: query.into(),
                kind,
                files_only,
                limit: 20,
                ..Default::default()
            };
            engine
                .search(&req)
                .hits
                .into_iter()
                .map(|h| h.entry.name)
                .collect()
        };
        let at = |names: &[String], name: &str| names.iter().position(|n| n == name);

        let safari = find("safari", KindFilter::Any, false);
        assert_eq!(safari[0], "Safari.app");
        assert!(at(&safari, "safari-bookmarks.html").is_some());
        assert!(at(&safari, "Info.plist").is_none());

        let term = find("term", KindFilter::Any, false);
        assert!(at(&term, "Terminal.app") < at(&term, "terminal-notes.txt"));
        assert!(at(&term, "terminal-notes.txt").is_some());

        assert_eq!(find("notes", KindFilter::Folder, false)[0], "Notes.app");
        assert_eq!(find("utilities", KindFilter::Folder, false), ["Utilities"]);
        assert!(at(&find("notes", KindFilter::Any, true), "Notes.app").is_none());
        assert_eq!(
            find("notes", KindFilter::Document, false),
            ["terminal-notes.txt"]
        );
        assert!(at(&find("notes *.txt", KindFilter::Any, false), "Notes.app").is_none());
        let scoped = engine.search(&SearchRequest {
            query: "safari".into(),
            scope: Some(home.clone()),
            limit: 20,
            ..Default::default()
        });
        assert!(scoped.hits.iter().all(|h| h.entry.name != "Safari.app"));

        let sizes = engine.bundle_sizes(&apps);
        assert_eq!(sizes.get(&apps.join("Safari.app")), Some(&5));
        assert_eq!(sizes.get(&apps.join("Notes.app")), Some(&0));
        assert!(!sizes.contains_key(&apps.join("Utilities")));
        assert!(engine.bundle_sizes(&home).is_empty());

        let listed = engine.list(true, Some("safari"), 0);
        let app = listed.iter().find(|i| i.path == apps.join("Safari.app"));
        assert_eq!(app.map(|i| i.root.as_path()), Some(apps.as_path()));

        // A new app is found on the next search; folder times can be coarse.
        std::thread::sleep(Duration::from_millis(20));
        std::fs::create_dir_all(apps.join("Utilities/Console.app")).unwrap();
        assert_eq!(find("console", KindFilter::Any, false), ["Console.app"]);

        engine.set_app_dirs(&[]);
        assert!(at(&find("safari", KindFilter::Any, false), "Safari.app").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_letter_app_hits_dont_flood() {
        let entries: Vec<PathBuf> = ["/Applications/Safari.app", "/System/Applications/Notes.app"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let words = vec!["s".to_string()];
        // fff-search scores one-letter matches 0, as `search_root` asks it.
        let mut hits = app_hits(&entries, &words);
        for file in ["/Users/me/s.txt", "/Users/me/Documents/sketch.txt"] {
            hits.push(RawHit::file(file.into(), 1, 0, None, 0));
        }
        for hit in &mut hits {
            hit.score = rerank(hit, &words);
        }
        hits.sort_by_key(|h| std::cmp::Reverse(h.score));
        let names: Vec<String> = hits.iter().map(|h| display_name(&h.path)).collect();
        assert_eq!(names, ["s.txt", "Safari.app", "sketch.txt", "Notes.app"]);
    }

    #[test]
    fn search_settings_change_content_search() {
        let dir = std::env::temp_dir().join(format!("fff-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "Hello there\n").unwrap();
        std::fs::write(dir.join("b.txt"), "hello there\n").unwrap();
        std::fs::write(dir.join("small.txt"), "needle\n").unwrap();
        let big = format!("needle\n{}\n", "x".repeat(2 * 1024 * 1024));
        std::fs::write(dir.join("big.txt"), big).unwrap();
        let engine = Engine::new(EngineOptions {
            roots: vec![dir.clone()],
            app_dirs: Vec::new(),
            db_dir: None,
            watch: false,
            search: Default::default(),
        });
        assert!(engine.wait_ready(Duration::from_secs(10)));
        let grep = |text: &str| {
            let mut names: Vec<String> = engine
                .grep(&GrepRequest {
                    patterns: vec![text.into()],
                    limit: 50,
                    ..Default::default()
                })
                .hits
                .iter()
                .map(|h| display_name(&h.path))
                .collect();
            names.sort();
            names
        };
        let set = |edit: &dyn Fn(&mut SearchSettings)| {
            let mut settings = engine.search_settings();
            edit(&mut settings);
            engine.set_search_settings(settings);
        };

        // Smart: lowercase ignores case, a capital letter matches it.
        assert_eq!(grep("hello"), ["a.txt", "b.txt"]);
        assert_eq!(grep("Hello"), ["a.txt"]);
        set(&|s| s.case = SearchCase::Sensitive);
        assert_eq!(grep("hello"), ["b.txt"]);
        set(&|s| s.case = SearchCase::Insensitive);
        assert_eq!(grep("Hello"), ["a.txt", "b.txt"]);

        assert_eq!(grep("needle"), ["big.txt", "small.txt"]);
        set(&|s| s.max_file_size_mb = 1);
        assert_eq!(grep("needle"), ["small.txt"]);

        set(&|s| s.max_file_size_mb = 10);
        assert_eq!(grep("needle"), ["big.txt", "small.txt"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

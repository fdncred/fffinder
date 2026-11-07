//! The one copy of the settings the app saves.
//!
//! Each browser (a tab or split pane) reads its own [`Config`], because some
//! settings are its own: one tab can show icons while another shows a list.
//! When a browser changes a setting it tells [`Settings`], which takes only
//! the fields that changed, shows the shared ones to every other browser, and
//! saves. So a browser can never write an old value over a newer one.

use fffinder_core::config::config_path;
use fffinder_core::folder_views::FolderViews;
use fffinder_core::listing::SortOrder;
use fffinder_core::{Config, ViewMode};
use gpui_kit::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long after the last change the settings are written.
const SAVE_DELAY: Duration = Duration::from_millis(400);

/// Every setting as last changed, by any browser.
pub struct Settings {
    config: Config,
    /// What this app last read from or wrote to the file.
    saved: Config,
    /// Write changes to the file. Tools and tests that drive a window turn
    /// this off so they never touch the user's settings.
    persist: bool,
    save_task: Task<()>,
    /// The view and sort chosen in each folder.
    folder_views: FolderViews,
    views_unsaved: bool,
    views_task: Task<()>,
    _quit: Subscription,
}

/// Where the app finds [`Settings`]. A global can't be observed for changes,
/// so it holds an entity, which can.
struct SettingsHandle(Entity<Settings>);

impl Global for SettingsHandle {}

/// Get the app's settings, read from the file the first time.
///
/// # Returns
///
/// The one [`Settings`] entity, which can be observed for changes
pub fn entity(cx: &mut App) -> Entity<Settings> {
    if let Some(handle) = cx.try_global::<SettingsHandle>() {
        return handle.0.clone();
    }
    let settings = cx.new(|cx| {
        let config = Config::load();
        Settings {
            saved: config.clone(),
            config,
            persist: true,
            save_task: Task::ready(()),
            folder_views: FolderViews::load(),
            views_unsaved: false,
            views_task: Task::ready(()),
            // A change made just before quitting hasn't been written yet.
            _quit: cx.on_app_quit(|this: &mut Settings, _| {
                this.save_now();
                async {}
            }),
        }
    });
    cx.set_global(SettingsHandle(settings.clone()));
    settings
}

/// Copy the settings as they are now.
///
/// # Returns
///
/// A copy of every setting as last changed, by any browser
pub fn current(cx: &mut App) -> Config {
    entity(cx).read(cx).config.clone()
}

/// Stop or start writing changes to the file.
///
/// # Arguments
///
/// * `persist` - Whether changes are written from now on
pub fn set_persist(persist: bool, cx: &mut App) {
    entity(cx).update(cx, |settings, _| settings.persist = persist);
}

/// Check whether changes are being written to the file.
///
/// # Returns
///
/// `true` unless a tool or test turned writing off
pub fn persists(cx: &mut App) -> bool {
    entity(cx).read(cx).persist
}

impl Settings {
    /// Read the settings as they are now.
    ///
    /// # Returns
    ///
    /// Every setting as last changed, by any browser
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// A browser went from `before` to `after`: take what changed, tell the
    /// other browsers, and save soon. With `shared_only`, its own view of the
    /// files stays its own, which is how split view's second pane works.
    pub(crate) fn take_changes(
        &mut self,
        before: &Config,
        after: &Config,
        shared_only: bool,
        cx: &mut Context<Self>,
    ) {
        if shared_only {
            self.config.take_shared_changes(before, after);
        } else {
            self.config.take_changes(before, after);
        }
        cx.notify();
        if !self.persist {
            return;
        }
        self.save_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Ok((saved, config)) =
                this.read_with(cx, |this, _| (this.saved.clone(), this.config.clone()))
            else {
                return;
            };
            let written = config.clone();
            let saving = cx.background_spawn(async move { save(&saved, &config) });
            // After a failed write `saved` stays as it was, so the next save
            // and the one at quit still see these changes as unwritten.
            if saving.await {
                this.update(cx, |this, _| this.saved = written).ok();
            }
        });
    }

    /// Take what changed in the file since this app last read or wrote it,
    /// such as a hand edit or a `--pick` run. Changes this app hasn't written
    /// yet are newer than the file, so with any of those it waits for the next
    /// time. A file that can't be read leaves everything as it is.
    pub(crate) fn reread_file(&mut self, cx: &mut Context<Self>) {
        if !self.persist || self.config != self.saved {
            return;
        }
        let Some(file) = Config::read_from(&config_path()) else {
            return;
        };
        if file != self.saved {
            self.config.clone_from(&file);
            self.saved = file;
            cx.notify();
        }
    }

    /// The view and sort last chosen in `folder`, if any.
    pub(crate) fn folder_view(&self, folder: &Path) -> Option<(ViewMode, SortOrder)> {
        self.folder_views.get(folder)
    }

    /// Remember the view and sort chosen in `folder`.
    pub(crate) fn remember_folder_view(
        &mut self,
        folder: PathBuf,
        view: ViewMode,
        sort: SortOrder,
        cx: &mut Context<Self>,
    ) {
        self.folder_views.set(&folder, view, sort);
        self.views_unsaved = true;
        if !self.persist {
            return;
        }
        self.views_task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let Ok(views) = this.update(cx, |this, _| {
                this.views_unsaved = false;
                this.folder_views.clone()
            }) else {
                return;
            };
            if !cx.background_spawn(async move { save_views(&views) }).await {
                this.update(cx, |this, _| this.views_unsaved = true).ok();
            }
        });
    }

    fn save_now(&mut self) {
        if !self.persist {
            return;
        }
        if self.config != self.saved && save(&self.saved, &self.config) {
            self.saved.clone_from(&self.config);
        }
        if self.views_unsaved && save_views(&self.folder_views) {
            self.views_unsaved = false;
        }
    }
}

/// Write the changes from `saved` to `config` into the settings file. Returns
/// whether that worked.
fn save(saved: &Config, config: &Config) -> bool {
    Config::save_changes(saved, config, &config_path())
        .inspect_err(|err| log::error!("saving settings: {err}"))
        .is_ok()
}

/// Write the folder views to their file. Returns whether that worked. Losing
/// them is harmless, so a failure only warns.
fn save_views(views: &FolderViews) -> bool {
    views
        .save()
        .inspect_err(|err| log::warn!("saving folder views: {err}"))
        .is_ok()
}

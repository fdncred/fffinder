//! The one copy of the settings the app saves.
//!
//! Each browser (a tab or split pane) reads its own [`Config`], because some
//! settings are its own: one tab can show icons while another shows a list.
//! When a browser changes a setting it tells [`Settings`], which takes only
//! the fields that changed, shows the shared ones to every other browser, and
//! saves. So a browser can never write an old value over a newer one.

use fffinder_core::Config;
use fffinder_core::config::config_path;
use gpui_kit::*;
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
    _quit: Subscription,
}

/// Where the app finds [`Settings`]. A global can't be observed for changes,
/// so it holds an entity, which can.
struct SettingsHandle(Entity<Settings>);

impl Global for SettingsHandle {}

/// The app's settings, read from the file the first time.
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

/// The settings as they are now.
pub fn current(cx: &mut App) -> Config {
    entity(cx).read(cx).config.clone()
}

/// Stop or start writing changes to the file.
pub fn set_persist(persist: bool, cx: &mut App) {
    entity(cx).update(cx, |settings, _| settings.persist = persist);
}

/// Whether changes are being written to the file.
pub fn persists(cx: &mut App) -> bool {
    entity(cx).read(cx).persist
}

impl Settings {
    /// The settings as they are now.
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
            saving.await;
            this.update(cx, |this, _| this.saved = written).ok();
        });
    }

    fn save_now(&mut self) {
        if self.persist && self.config != self.saved {
            save(&self.saved, &self.config);
            self.saved.clone_from(&self.config);
        }
    }
}

/// Write the changes from `saved` to `config` into the settings file.
fn save(saved: &Config, config: &Config) {
    if let Err(err) = Config::save_changes(saved, config, &config_path()) {
        log::warn!("saving settings: {err}");
    }
}

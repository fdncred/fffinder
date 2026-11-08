//! Asking for Full Disk Access on macOS: System Settings opens at the right
//! pane, and a dialog says what to turn on there. See `crate::disk_access`
//! for how fffinder tells whether it has it.

use super::Finder;
use crate::disk_access::{self, Access};
use crate::theme::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::*;

/// What to turn on in System Settings: fffinder itself when it runs from its
/// app bundle, or else the app that started it, since macOS gives that app's
/// permissions to everything it starts.
fn what_to_turn_on() -> &'static str {
    if disk_access::in_app_bundle() {
        "Turn on fffinder there, adding it with the + button if it isn’t in the list. \
         Then quit and reopen fffinder."
    } else {
        "fffinder was started from another app, such as a terminal, and macOS gives it \
         that app’s access. Turn on that app there, adding it with the + button if it \
         isn’t in the list. Then quit and reopen that app and fffinder."
    }
}

impl Finder {
    /// Open System Settings at Full Disk Access and say in a dialog what to
    /// turn on there, with a way to stop asking.
    ///
    /// # Arguments
    ///
    /// * `window` - The window the dialog opens in
    /// * `cx` - The browser's context
    pub fn ask_for_full_disk_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        crate::system::open_url(disk_access::SETTINGS_URL, cx);
        self.focus_before_dialog(window, cx);
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let muted = cx.theme().muted_foreground;
            dialog
                .title("Give fffinder Full Disk Access")
                .w(ui(460.))
                // System Settings comes up in front, and the click that
                // brings the window back would otherwise land outside the
                // dialog and close it before it's read.
                .overlay_closable(false)
                .child(
                    v_flex()
                        .id("disk-access-text")
                        .gap(ui(8.))
                        .text_size(ui(13.))
                        .child(
                            "macOS keeps fffinder out of the Trash, Mail, Safari, and other \
                             protected places until it has Full Disk Access, so those folders \
                             look empty and searches miss what’s inside them.",
                        )
                        .child("System Settings is open at Privacy & Security › Full Disk Access.")
                        .child(div().text_color(muted).child(what_to_turn_on())),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("disk-access-never")
                                .label("Don’t Ask Again")
                                .on_click(window.listener_for(
                                    &view,
                                    |this: &mut Finder, _: &ClickEvent, window, cx| {
                                        this.set_ask_for_full_disk_access(false, cx);
                                        window.close_dialog(cx);
                                    },
                                )),
                        )
                        .child(
                            Button::new("disk-access-open")
                                .label("Open System Settings")
                                .on_click(|_, _, cx| {
                                    crate::system::open_url(disk_access::SETTINGS_URL, cx)
                                }),
                        )
                        .child(
                            DialogClose::new()
                                .child(Button::new("disk-access-ok").primary().label("OK")),
                        ),
                )
        });
    }

    /// Choose whether startup asks for Full Disk Access when it's missing.
    ///
    /// # Arguments
    ///
    /// * `ask` - Whether to ask
    /// * `cx` - The browser's context
    pub fn set_ask_for_full_disk_access(&mut self, ask: bool, cx: &mut Context<Self>) {
        self.config.ask_for_full_disk_access = ask;
        self.settings_changed(cx);
        cx.notify();
    }
}

/// Settings › Privacy on macOS: whether fffinder has Full Disk Access, a
/// button to the System Settings pane, and whether startup asks.
///
/// # Arguments
///
/// * `finder` - The browser whose settings show
/// * `view` - The same browser, for the controls to change
/// * `muted` - The color of the explanations
/// * `window` - The window Settings is in
/// * `cx` - The app, which holds what the startup check found
///
/// # Returns
///
/// The section's rows
pub(super) fn settings_controls(
    finder: &Finder,
    view: &Entity<Finder>,
    muted: Hsla,
    window: &mut Window,
    cx: &App,
) -> impl IntoElement {
    // macOS applies a change only when fffinder starts again, so what the
    // check found at startup is still the answer.
    let state = match cx.try_global::<disk_access::Checked>().map(|c| c.0) {
        Some(Access::Granted) => "On: fffinder can read every folder",
        Some(Access::Denied) => "Off: the Trash and other apps’ data stay closed to fffinder",
        Some(Access::Unknown) | None => "Lets fffinder read the Trash and other apps’ data",
    };
    v_flex()
        .gap(ui(8.))
        .child(super::dialogs::setting_row(
            "Full Disk Access",
            state,
            Button::new("disk-access-settings")
                .small()
                .label("Open System Settings…")
                .on_click(|_, _, cx| crate::system::open_url(disk_access::SETTINGS_URL, cx)),
            muted,
        ))
        .child(
            Switch::new("ask-for-disk-access")
                .small()
                .label("Ask at startup when it’s off")
                .checked(finder.config.ask_for_full_disk_access)
                .on_change(
                    window.listener_for(view, |this: &mut Finder, on: &bool, _, cx| {
                        this.set_ask_for_full_disk_access(*on, cx)
                    }),
                ),
        )
}

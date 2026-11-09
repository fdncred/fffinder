//! Asking for Full Disk Access on macOS: a dialog says what to turn on in
//! System Settings and opens it at the right pane. Indexing waits for the
//! answer, since without access it makes macOS ask about each protected
//! folder. See `crate::disk_access` for how fffinder tells whether it has it.

use super::Finder;
use crate::disk_access::{self, Access};
use crate::theme::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
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
    /// Say in a dialog what to turn on in System Settings for Full Disk
    /// Access, with a button that opens it there, a way to index without it
    /// while indexing waits, and a way to stop asking.
    ///
    /// # Arguments
    ///
    /// * `window` - The window the dialog opens in
    /// * `cx` - The browser's context
    pub fn ask_for_full_disk_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_before_dialog(window, cx);
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let muted = cx.theme().muted_foreground;
            let held = disk_access::index_held(cx);
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
                        .when(held, |this| {
                            this.child(
                                "Indexing waits until you decide. Without Full Disk Access, \
                                 macOS asks about Desktop, Documents, Downloads, and other \
                                 folders one at a time while fffinder indexes them.",
                            )
                        })
                        .child("Open System Settings to Privacy & Security › Full Disk Access.")
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
                        .when(held, |footer| {
                            footer.child(
                                Button::new("disk-access-index")
                                    .label("Index Without It")
                                    .on_click(window.listener_for(
                                        &view,
                                        |this: &mut Finder, _: &ClickEvent, window, cx| {
                                            this.start_held_index(cx);
                                            window.close_dialog(cx);
                                        },
                                    )),
                            )
                        })
                        .child(
                            Button::new("disk-access-open")
                                .primary()
                                .label("Open System Settings")
                                .on_click(|_, _, cx| {
                                    crate::system::open_url(disk_access::SETTINGS_URL, cx)
                                }),
                        )
                        .when(!held, |footer| {
                            footer.child(
                                DialogClose::new().child(Button::new("disk-access-ok").label("OK")),
                            )
                        }),
                )
        });
    }

    /// Start the indexing that waited for Full Disk Access, when it did.
    ///
    /// # Arguments
    ///
    /// * `cx` - The browser's context
    pub fn start_held_index(&mut self, cx: &mut Context<Self>) {
        if !disk_access::index_held(cx) {
            return;
        }
        cx.set_global(disk_access::IndexHeld(false));
        self.index_search_roots(cx);
        // Every window's sidebar says indexing waits; now it doesn't.
        cx.refresh_windows();
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
        // Not asking means not waiting either.
        if !ask {
            self.start_held_index(cx);
        }
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

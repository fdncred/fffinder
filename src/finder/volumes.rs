//! Volumes and servers: ejecting from the sidebar, the free space in the
//! status bar, and Connect to Server.

use super::Finder;
use crate::actions::ConnectToServer;
use crate::model::Location;
use crate::theme::ui;
use fffinder_core::entry::display_name;
use fffinder_core::places;
use fffinder_core::servers::Server;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogClose, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::NotificationType;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::*;
use std::path::{Path, PathBuf};

/// How many servers Connect to Server remembers.
const RECENT_SERVERS: usize = 10;

impl Finder {
    /// Read the free space on the current folder's volume, off the UI thread.
    pub(super) fn load_free_space(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.current_folder().map(Path::to_path_buf) else {
            self.free_space = None;
            return;
        };
        cx.spawn(async move |this, cx| {
            let free = cx
                .background_spawn(async move { places::free_space(&dir) })
                .await;
            this.update(cx, |this, cx| {
                this.free_space = free;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Eject the disk, or unmount the server, at `path`.
    pub(super) fn eject(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let task = crate::system::eject(&path, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        // Don't stay in a folder that's gone.
                        if this.current_folder().is_some_and(|f| f.starts_with(&path))
                            && let Some(home) = dirs::home_dir()
                        {
                            this.navigate(Location::Folder(home), window, cx);
                        }
                        window.push_notification(format!("Ejected “{}”", display_name(&path)), cx);
                    }
                    Err(err) => window.push_notification(
                        (
                            NotificationType::Error,
                            format!("Couldn’t eject “{}”: {err}", display_name(&path)),
                        ),
                        cx,
                    ),
                }
                this.locations = places::locations();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Cmd+K: type a server's address, or pick a recent one, and connect.
    pub(super) fn connect_to_server(
        &mut self,
        _: &ConnectToServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_before_dialog(window, cx);
        let recent = self.config.recent_servers.clone();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("smb://server/share, sftp://me@host/path, https://host/dav")
                .default_value(recent.first().cloned().unwrap_or_default())
        });
        let view = cx.entity();
        let first = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if first.replace(false) {
                input.update(cx, |s, cx| {
                    s.focus(window, cx);
                    s.select_all(window, cx);
                });
            }
            let muted = cx.theme().muted_foreground;
            let (ok_input, button_input) = (input.clone(), input.clone());
            let ok_view = view.clone();
            let mut body = v_flex()
                .gap(ui(8.))
                .child(Input::new(&input).id("server-input"))
                .child(
                    div()
                        .text_size(ui(11.))
                        .text_color(muted)
                        .child(server_help()),
                );
            if !recent.is_empty() {
                body = body.child(
                    div()
                        .pt(ui(4.))
                        .text_size(ui(11.))
                        .text_color(muted)
                        .child("Recent servers"),
                );
                for (ix, address) in recent.iter().enumerate() {
                    let (fill, address) = (input.clone(), address.clone());
                    body = body.child(
                        Button::new(("recent-server", ix))
                            .ghost()
                            .small()
                            .label(address.clone())
                            .on_click(move |_, window, cx| {
                                fill.update(cx, |s, cx| s.set_value(address.clone(), window, cx))
                            }),
                    );
                }
            }
            dialog
                .title("Connect to Server")
                .w(ui(480.))
                .child(body)
                .on_ok(move |_, window, cx| {
                    ok_view.update(cx, |this, cx| this.submit_server(&ok_input, window, cx))
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(Button::new("server-cancel").label("Cancel")),
                        )
                        .child(
                            Button::new("server-connect")
                                .primary()
                                .label("Connect")
                                .on_click(window.listener_for(
                                    &view,
                                    move |this: &mut Finder, _: &ClickEvent, window, cx| {
                                        if this.submit_server(&button_input, window, cx) {
                                            window.close_dialog(cx);
                                            this.focus_files(window, cx);
                                        }
                                    },
                                )),
                        ),
                )
        });
    }

    /// Connect to the typed server, then open it. Returns whether the
    /// dialog closes: it stays open for an address that can't work.
    fn submit_server(
        &mut self,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let address = input.read(cx).value().trim().to_string();
        let server = match Server::parse(&address) {
            Ok(server) => server,
            Err(problem) => {
                window.push_notification((NotificationType::Warning, problem), cx);
                return false;
            }
        };
        window.push_notification(format!("Connecting to {}…", server.host()), cx);
        let task = crate::system::connect(&address, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(path) => {
                    let recent = &mut this.config.recent_servers;
                    recent.retain(|a| *a != address);
                    recent.insert(0, address.clone());
                    recent.truncate(RECENT_SERVERS);
                    this.schedule_save(cx);
                    this.locations = places::locations();
                    this.navigate(Location::Folder(path), window, cx);
                }
                Err(err) => window.push_notification(
                    (
                        NotificationType::Error,
                        format!("Couldn’t connect to {address}: {err}"),
                    ),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
        true
    }
}

fn server_help() -> &'static str {
    if cfg!(target_os = "macos") {
        "SMB, AFP, NFS, WebDAV (https://), and FTP. macOS asks for a password when the server needs one."
    } else if cfg!(windows) {
        "SMB file shares (smb://server/share or \\\\server\\share) and WebDAV (https://)."
    } else {
        "SMB, SFTP, WebDAV (https://), AFP, NFS, and FTP, through GIO. Passwords come from your keyring."
    }
}

//! Quick Look in a window of its own, as in Finder: it can be moved,
//! resized, maximized, or made full screen, and follows the browser's
//! selection until it's closed. In `--term` mode, which draws a single
//! window into the terminal, Quick Look is a dialog instead (see
//! `preview.rs`).

use super::Finder;
use super::pdf::Surface;
use super::preview::{preview_area, preview_caption, text_size_buttons, text_size_handler};
use crate::actions::{
    MoveDown, MoveLeft, MoveRight, MoveUp, NextPdfPage, PreviewTextBigger, PreviewTextReset,
    PreviewTextSmaller, PreviousPdfPage, QUICK_LOOK, QuickLook,
};
use crate::theme::ui;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub struct QuickLookWindow {
    finder: WeakEntity<Finder>,
    /// The browser's window: what the browser does on Quick Look's behalf,
    /// such as moving its selection, happens there.
    finder_window: AnyWindowHandle,
    focus: FocusHandle,
    _subscriptions: [Subscription; 2],
}

impl QuickLookWindow {
    fn new(
        finder: &Entity<Finder>,
        finder_window: AnyWindowHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> QuickLookWindow {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let subscriptions = [
            // Redraw as the browser's selection and previews change.
            cx.observe(finder, |_, _, cx| cx.notify()),
            // A browser that closes takes its Quick Look with it.
            cx.observe_release_in(finder, window, |_, _, window, _| window.remove_window()),
        ];
        QuickLookWindow {
            finder: finder.downgrade(),
            finder_window,
            focus,
            _subscriptions: subscriptions,
        }
    }

    /// Run `f` on the browser, in the browser's window.
    fn with_finder(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut Finder, &mut Window, &mut Context<Finder>),
    ) {
        let Some(finder) = self.finder.upgrade() else {
            return;
        };
        self.finder_window
            .update(cx, |_, window, cx| {
                finder.update(cx, |this, cx| f(this, window, cx))
            })
            .ok();
    }

    fn close(&mut self, window: &mut Window, cx: &mut App) {
        window.remove_window();
        self.finder_window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }

    fn page(&mut self, by: isize, cx: &mut Context<Self>) {
        self.with_finder(cx, |finder, _, cx| {
            finder.step_pdf_page(by, Surface::QuickLook, cx)
        });
    }

    /// The arrow keys move the browser's selection, as with Quick Look in
    /// Finder.
    fn step(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        self.with_finder(cx, |finder, window, cx| {
            finder.quick_look_step(dx, dy, window, cx)
        });
    }
}

impl Render for QuickLookWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (background, border) = (theme.background, theme.border);
        let root = v_flex()
            .id("quick-look")
            .size_full()
            .bg(background)
            .track_focus(&self.focus)
            .key_context(QUICK_LOOK)
            .on_action(cx.listener(|this, _: &QuickLook, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.step(0, -1, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.step(0, 1, cx)))
            .on_action(cx.listener(|this, _: &MoveLeft, _, cx| this.step(-1, 0, cx)))
            .on_action(cx.listener(|this, _: &MoveRight, _, cx| this.step(1, 0, cx)))
            .on_action(cx.listener(|this, _: &NextPdfPage, _, cx| this.page(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousPdfPage, _, cx| this.page(-1, cx)));
        let Some(finder) = self.finder.upgrade() else {
            return root;
        };
        let root = root
            .on_action(text_size_handler::<PreviewTextBigger>(&finder, Some(1.)))
            .on_action(text_size_handler::<PreviewTextSmaller>(&finder, Some(-1.)))
            .on_action(text_size_handler::<PreviewTextReset>(&finder, None));
        let browser = finder.read(cx);
        let Some(entry) = browser.primary_entry() else {
            return root;
        };
        window.set_window_title(&entry.name);
        let text_size = browser
            .preview_of(&entry)
            .has_text()
            .then(|| browser.config.preview_font_size());
        let (open_path, reveal_path) = (entry.path.clone(), entry.path.clone());
        let (open_finder, finder_window) = (finder.downgrade(), self.finder_window);
        root.child(div().flex_1().min_h_0().w_full().child(preview_area(
            browser,
            &finder,
            &entry,
            Surface::QuickLook,
            cx,
        )))
        .child(
            h_flex()
                .flex_shrink_0()
                .gap(ui(8.))
                .px(ui(10.))
                .py(ui(6.))
                .border_t_1()
                .border_color(border)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(preview_caption(browser, &entry, cx)),
                )
                .when_some(text_size, |this, size| {
                    this.child(text_size_buttons(&finder, size))
                })
                .child(
                    Button::new("ql-reveal")
                        .small()
                        .label(super::menus::reveal_label())
                        .on_click(move |_, _, cx| crate::system::reveal(&reveal_path, cx)),
                )
                .child(
                    Button::new("ql-open")
                        .small()
                        .primary()
                        .label("Open")
                        .on_click(move |_, window, cx| {
                            window.remove_window();
                            let (finder, path) = (open_finder.clone(), open_path.clone());
                            finder_window
                                .update(cx, |_, window, cx| {
                                    window.activate_window();
                                    finder
                                        .update(cx, |this, cx| {
                                            // A file no app claims opens in an editor.
                                            this.open_files(vec![path], window, cx)
                                        })
                                        .ok();
                                })
                                .ok();
                        }),
                ),
        )
    }
}

impl Finder {
    /// The Quick Look window, while it's open.
    pub fn quick_look_window(&self) -> Option<AnyWindowHandle> {
        self.quick_look_window
    }

    /// Open Quick Look in a window of its own over this one, or close it.
    pub(super) fn toggle_quick_look_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = self.quick_look_window.take() {
            self.quick_look_open = false;
            open.update(cx, |_, window, _| window.remove_window()).ok();
            return;
        }
        let Some(entry) = self.primary_entry() else {
            return;
        };
        self.ensure_preview(&entry, cx);
        self.ensure_pdf(&entry, cx);
        // Most of the browser's window, centered over it, on its display.
        let parent = window.bounds();
        let size = size(
            (parent.size.width * 0.8).max(px(560.)),
            (parent.size.height * 0.85).max(px(400.)),
        );
        let origin = parent.center() - point(size.width / 2., size.height / 2.);
        let mut options = crate::window_options(entry.name.clone().into(), cx);
        options.window_bounds = Some(WindowBounds::Windowed(Bounds { origin, size }));
        options.window_min_size = Some(gpui_kit::size(px(400.), px(300.)));
        options.display_id = window.display(cx).map(|display| display.id());
        options.titlebar = Some(TitlebarOptions {
            title: Some(entry.name.into()),
            appears_transparent: false,
            traffic_light_position: None,
        });
        options.app_owns_titlebar_drag = false;
        let (finder, finder_window) = (cx.entity(), window.window_handle());
        self.quick_look_open = true;
        // A new window draws its first frame at once, which reads this
        // browser, so it opens after this update.
        cx.defer(move |cx| {
            let opened = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| QuickLookWindow::new(&finder, finder_window, window, cx))
            });
            finder.update(cx, |this, cx| match opened {
                Ok((handle, view)) => {
                    this.quick_look_window = Some(handle);
                    cx.observe_release(&view, |this, _, cx| {
                        this.quick_look_window = None;
                        this.quick_look_open = false;
                        this.forget_pdf_surface(Surface::QuickLook);
                        cx.notify();
                    })
                    .detach();
                }
                Err(err) => {
                    this.quick_look_open = false;
                    log::error!("opening Quick Look: {err}");
                }
            });
        });
    }
}

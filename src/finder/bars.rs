//! Search scope bar, path bar, status bar, and the picker's choose bar.

use super::{Finder, SearchScope, group_digits};
use crate::actions::*;
use crate::model::Location;
use crate::theme::palette;
use crate::theme::ui;
use fffinder_core::{KindFilter, ViewMode, places};
use gpui_kit::TestSupportExt as _;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::slider::Slider;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

impl Finder {
    pub(super) fn render_scope_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let accent = theme.primary;
        let muted = theme.muted_foreground;
        let bar = theme.tab_bar;
        let border = theme.border;
        let hover = theme.list_hover;
        let selected_bg = accent.opacity(0.14);

        let chip = |id: &'static str, label: SharedString, on: bool| {
            div()
                .id(id)
                .px(ui(10.))
                .py(ui(3.))
                .rounded(ui(6.))
                .text_color(if on { accent } else { muted })
                .font_weight(if on {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .when(on, |this| this.bg(selected_bg))
                .when(!on, |this| this.hover(|s| s.bg(hover)))
                .child(label)
        };

        let folder_name = self
            .current_folder()
            .map(fffinder_core::entry::display_name);
        let scope = self.scope;
        let contents = self.search_contents;
        let kind = self.config.kind_filter;
        let view = cx.entity();

        h_flex()
            .h(ui(36.))
            .flex_shrink_0()
            .px(ui(16.))
            .gap(ui(10.))
            .bg(bar)
            .border_b_1()
            .border_color(border)
            .text_size(ui(12.))
            .child(div().text_color(muted).child("Search:"))
            .child(
                chip(
                    "scope-computer",
                    "This Computer".into(),
                    scope == SearchScope::Computer,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.set_scope(SearchScope::Computer, cx)
                }))
                .test_support(),
            )
            .when_some(folder_name, |this, name| {
                this.child(
                    chip(
                        "scope-folder",
                        format!("“{name}”").into(),
                        scope == SearchScope::Folder,
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.set_scope(SearchScope::Folder, cx)
                    }))
                    .test_support(),
                )
            })
            .child(div().w(ui(1.)).h(ui(16.)).bg(border))
            .child(
                chip("by-name", "Name".into(), !contents)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.set_search_contents(false, cx)
                    }))
                    .test_support(),
            )
            .child(
                chip("by-contents", "Contents".into(), contents)
                    .on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.set_search_contents(true, cx)
                        }),
                    )
                    .test_support(),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("filter-toggle")
                    .p(ui(3.))
                    .rounded(ui(5.))
                    .hover(|s| s.bg(hover))
                    .when(self.filter_bar_open(), |this| this.bg(selected_bg))
                    .child(Icon::new(Lucide::Funnel).with_size(ui(14.)).text_color(
                        if self.filter_bar_open() {
                            accent
                        } else {
                            muted
                        },
                    ))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("Filter with Nushell")
                            .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.toggle_filter(&crate::actions::ToggleFilter, window, cx)
                    }))
                    .test_support(),
            )
            .child(
                div()
                    .id("search-help")
                    .p(ui(3.))
                    .rounded(ui(5.))
                    .hover(|s| s.bg(hover))
                    .child(
                        Icon::new(IconName::Info)
                            .with_size(ui(14.))
                            .text_color(muted),
                    )
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.search_help(&SearchHelp, window, cx)
                    }))
                    .test_support(),
            )
            .child(
                Button::new("kind")
                    .outline()
                    .xsmall()
                    .w(ui(130.))
                    .label(kind.label())
                    .icon(Icon::new(IconName::ChevronsUpDown))
                    .disabled(contents)
                    .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, window, _| {
                        for option in KindFilter::ALL {
                            menu = menu.item(
                                PopupMenuItem::new(option.label())
                                    .checked(option == kind)
                                    .on_click(window.listener_for(
                                        &view,
                                        move |this: &mut Finder, _: &ClickEvent, _, cx| {
                                            this.set_kind(option, cx)
                                        },
                                    )),
                            );
                        }
                        menu.min_w(ui(150.))
                    }),
            )
    }

    /// The path shown in the bar: the selected item while searching or in
    /// Recents, otherwise the current folder.
    fn path_bar_target(&self) -> Option<(PathBuf, bool)> {
        let selected = self.primary_entry();
        let detached = self.is_searching() || self.current_folder().is_none();
        match (detached, selected) {
            (true, Some(entry)) => Some((entry.path, !entry.is_dir)),
            (true, None) => None,
            (false, _) => self.current_folder().map(|p| (p.to_path_buf(), false)),
        }
    }

    pub(super) fn render_path_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let bar = theme.status_bar;
        let border = theme.status_bar_border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let hover = theme.list_hover;
        let pal = palette(cx);

        let mut crumbs = h_flex().gap(ui(5.)).overflow_hidden();
        if let Some((target, is_file)) = self.path_bar_target() {
            let parts = places::breadcrumbs(&target);
            let last = parts.len().saturating_sub(1);
            for (ix, (name, path)) in parts.into_iter().enumerate() {
                if ix > 0 {
                    crumbs = crumbs.child(
                        Icon::new(IconName::ChevronRight)
                            .with_size(ui(9.))
                            .text_color(muted),
                    );
                }
                let icon = if ix == 0 {
                    Icon::new(IconName::HardDrive)
                } else if ix == last && is_file {
                    Icon::new(IconName::File)
                } else {
                    Icon::new(IconName::Folder)
                };
                let is_last = ix == last;
                crumbs = crumbs.child(
                    h_flex()
                        .id(("crumb", ix))
                        .gap(ui(4.))
                        .px(ui(3.))
                        .rounded(ui(4.))
                        .flex_shrink(if is_last { 0. } else { 1. })
                        .min_w(ui(20.))
                        .hover(|s| s.bg(hover))
                        .child(icon.with_size(ui(12.)).text_color(if ix == 0 {
                            muted
                        } else {
                            pal.sidebar_icon
                        }))
                        .child(
                            div()
                                .truncate()
                                .text_color(if is_last { fg } else { muted })
                                .child(name),
                        )
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            let path = path.clone();
                            if is_last && is_file {
                                this.reveal_in_place(&path, window, cx);
                            } else {
                                this.navigate(Location::Folder(path), window, cx);
                            }
                        }))
                        .test_support(),
                );
            }
        }
        h_flex()
            .h(ui(28.))
            .flex_shrink_0()
            .px(ui(12.))
            .bg(bar)
            .border_t_1()
            .border_color(border)
            .text_size(ui(11.))
            .child(crumbs)
    }

    pub(super) fn render_status_bar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.picker.is_some() {
            return self.render_picker_bar(cx).into_any_element();
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let bar = theme.status_bar;
        let count = self.rows.len();
        let selected = self.selection.len();
        let filtered = !self.filter_text().is_empty() && self.filter_error().is_none();
        // Searches stop at the result limit; say so rather than imply a total.
        let before = self.unfiltered_len;
        let capped = self.is_searching() && before >= self.search_limit.max(1);
        let plus = if capped { "+" } else { "" };
        let text = if selected > 0 {
            format!("{selected} of {} selected", group_digits(count))
        } else if filtered {
            format!(
                "{} of {}{plus} items pass the filter",
                group_digits(count),
                group_digits(before)
            )
        } else if count == 1 {
            "1 item".to_string()
        } else {
            format!("{}{plus} items", group_digits(count))
        };
        let timing = self
            .search_stats
            .as_ref()
            .filter(|_| self.is_searching())
            .map(|stats| {
                let ms = format!("{:.1} ms", stats.elapsed.as_secs_f64() * 1000.);
                if stats.stopped {
                    format!("{ms}, stopped at the time limit")
                } else {
                    ms
                }
            });
        // The sidebar footer shows indexing progress when it is visible.
        let indexing = self
            .indexing_progress()
            .filter(|_| !self.config.show_sidebar);
        let icons = self.effective_view() == ViewMode::Icons;
        let fps = self.fps.as_ref().and_then(|meter| meter.fps());
        h_flex()
            .h(ui(28.))
            .flex_shrink_0()
            .px(ui(14.))
            .gap(ui(8.))
            .bg(bar)
            .text_size(ui(11.))
            .text_color(muted)
            .child(text)
            .when_some(timing, |this, timing| {
                this.child("·").child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(ui(10.5))
                        .child(timing),
                )
            })
            .when(self.searching || self.is_filtering(), |this| {
                this.child(Spinner::new().xsmall().color(muted))
            })
            .when_some(indexing, |this, n| {
                this.child(
                    h_flex()
                        .gap(ui(6.))
                        .child(Spinner::new().xsmall().color(muted))
                        .child(format!("Indexing {} files…", group_digits(n))),
                )
            })
            .child(div().flex_1())
            .when_some(fps, |this, fps| {
                this.child(
                    div()
                        .id("fps")
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(ui(10.5))
                        .child(format!("{fps} fps"))
                        .test_support(),
                )
            })
            .when(icons, |this| {
                this.child(
                    h_flex()
                        .gap(ui(6.))
                        .child(
                            Icon::new(IconName::Square)
                                .with_size(ui(9.))
                                .text_color(muted),
                        )
                        .child(Slider::new(&self.icon_slider).w(ui(100.)))
                        .child(
                            Icon::new(IconName::Square)
                                .with_size(ui(13.))
                                .text_color(muted),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_picker_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let multiple = self.picker.is_some_and(|p| p.multiple);
        let count = self.selected_entries().len();
        let hint = if multiple {
            "Select one or more items, then press Return to choose. Esc cancels."
        } else {
            "Select an item, then press Return to choose. Esc cancels."
        };
        h_flex()
            .h(ui(44.))
            .flex_shrink_0()
            .px(ui(14.))
            .gap(ui(8.))
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .text_size(ui(12.))
            .text_color(theme.muted_foreground)
            .child(Icon::new(Lucide::Terminal).with_size(ui(14.)))
            .child(div().flex_1().truncate().child(hint))
            .child(
                Button::new("picker-cancel")
                    .small()
                    .label("Cancel")
                    .on_click(|_, _, _| crate::picker::cancel()),
            )
            .child(
                Button::new("picker-choose")
                    .small()
                    .primary()
                    .label(if count > 1 {
                        format!("Choose {count} Items")
                    } else {
                        "Choose".to_string()
                    })
                    .disabled(count == 0)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.choose(&Choose, window, cx)
                    })),
            )
    }
}

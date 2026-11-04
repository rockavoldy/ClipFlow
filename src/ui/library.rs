//! Clip library list.
//!
//! Replaces `GalleryGrid` + `useVirtualGrid`. The old component windowed a
//! multi-column grid of cards with a `useVirtualGrid` DOM observer; GPUI's
//! `uniform_list` gives the same virtualisation for free as long as every row
//! has the same height, which is why the library is a list of wide rows instead
//! of a masonry grid. That is the one visual downgrade in phase 1: fewer clips
//! per screen, in exchange for smooth scrolling at thousands of files.
//!
//! Thumbnails are decoded lazily per visible row through
//! [`crate::ui::thumbs::ThumbnailCache`]; the first paint shows a placeholder
//! and the row re-renders when the texture lands.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::*;

use crate::media::library::ClipMetadata;
use crate::policy::gallery::{self, SortKey};
use crate::ui::deck::{Deck, Intent};
use crate::ui::theme::Palette;

/// Row height. Fixed, because `uniform_list` measures one row and assumes the
/// rest match.
pub const ROW_H: f32 = 64.0;
/// Wide enough for a 16:9 thumbnail at 96 px tall.
const THUMB_W: f32 = 96.0;

impl Deck {
    pub(crate) fn render_library(
        &self,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let games = gallery::available_games(&self.clips);
        let count = self.visible.len();
        let selected = self.selection.count();

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.cyan)
                            .child("LIBRARY"),
                    )
                    .child(
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.muted)
                            .child(format!("{count} clips")),
                    )
                    .children((selected > 0).then(|| {
                        div()
                            .font_family(crate::ui::theme::MONO)
                            .text_xs()
                            .text_color(palette.lime)
                            .child(format!("{selected} selected"))
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("folder")
                            .ghost()
                            .compact()
                            .label("Folder")
                            .tooltip("Open the clips folder")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                deck.handle_intent(Intent::OpenClipsFolder, cx);
                            })),
                    )
                    .child(
                        Button::new("reveal")
                            .ghost()
                            .compact()
                            .label("Reveal")
                            .tooltip("Show the selection in Explorer")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                let paths = deck.selection.selected.clone();
                                if let Some(first) = paths.first() {
                                    deck.reveal_clip(first, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("copy")
                            .ghost()
                            .compact()
                            .label("Copy")
                            .tooltip("Put the selected clips on the clipboard")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                let paths = deck.selection.selected.clone();
                                if !paths.is_empty() {
                                    deck.copy_paths(paths, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new("delete")
                            .ghost()
                            .compact()
                            .label("Delete")
                            .tooltip("Delete the selected clips from disk")
                            .on_click(cx.listener(|deck, _ev, _window, cx| {
                                if deck.selection.count() > 0 {
                                    deck.delete_selected(cx);
                                }
                            })),
                    ),
            );

        // Game filter: "All" plus every game present in the library.
        let active_game = self.gallery_state.game_filter.clone();
        let game_chips = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(self.filter_chip(
                palette,
                "All",
                active_game == gallery::ALL_GAMES,
                cx,
                String::from("__all"),
            ))
            .children(games.into_iter().map(|game| {
                let active = active_game == game;
                let key = format!("game-{game}");
                self.filter_chip(palette, &game_label(&game), active, cx, key)
                    .on_click(cx.listener(move |deck, _ev, _window, cx| {
                        deck.set_game_filter(game.clone(), cx);
                    }))
            }));

        let sort_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .children(SortKey::ALL.into_iter().map(|key| {
                let active = self.gallery_state.sort_key == key;
                let label = sort_label(key);
                {
                    let mut chip = div()
                        .id(format!("sort-{}", key.as_str()))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .cursor_pointer()
                        .font_family(crate::ui::theme::MONO)
                        .text_xs()
                        .text_color(if active { palette.cyan } else { palette.muted })
                        .child(label);
                    if active {
                        chip = chip.bg(palette.panel_2);
                    }
                    chip.on_click(cx.listener(move |deck, _ev, _window, cx| {
                        deck.set_sort(key, cx);
                    }))
                }
            }));

        let fav_on = self.gallery_state.fav_only;
        let audio_on = self.gallery_state.audio_only;
        let toggles = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .child(
                self.filter_chip(palette, "Favourites", fav_on, cx, "__fav".into())
                    .on_click(cx.listener(|deck, _ev, _window, cx| {
                        deck.toggle_fav_filter(cx);
                    })),
            )
            .child(
                self.filter_chip(palette, "With audio", audio_on, cx, "__audio".into())
                    .on_click(cx.listener(|deck, _ev, _window, cx| {
                        deck.toggle_audio_filter(cx);
                    })),
            );

        let list: AnyElement = if self.loading_clips {
            div()
                .p_6()
                .font_family(crate::ui::theme::MONO)
                .text_xs()
                .text_color(palette.muted)
                .child("Scanning the clips folder…")
                .into_any_element()
        } else if self.visible.is_empty() {
            empty_state(palette, self.clips.is_empty()).into_any_element()
        } else {
            let card = cx.entity();
            let visible: Rc<Vec<ClipMetadata>> = Rc::new(self.visible.clone());
            let selection = self.selection.clone();

            uniform_list(
                "library-list",
                visible.len(),
                move |range, _window, cx| {
                    let visible = Rc::clone(&visible);
                    let selection = selection.clone();
                    let card = card.clone();
                    range
                        .map(|ix| {
                            let clip = &visible[ix];
                            let path = clip.path.clone();
                            let path_for_click = path.clone();
                            let path_for_open = path.clone();
                            let path_for_star = path.clone();
                            let selected = selection.is_selected(&path);
                            let favorite = clip.favorite;

                            let thumb = card.read(cx).thumbs.get_cached(&path);

                            let mut row = div()
                                .id(("clip", ix))
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .h(px(ROW_H))
                                .w_full()
                                .px_3()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(move |style| style.bg(palette.panel_2))
                                // Shift/Ctrl click drives the reducer, and the
                                // reducer ranges over the FULL visible order so
                                // a shift-click can select rows that are not
                                // painted yet.
                                .on_mouse_down(MouseButton::Left, {
                                    let card = card.clone();
                                    move |ev: &MouseDownEvent, _window, cx| {
                                        let path = path_for_click.clone();
                                        let (ctrl, shift) =
                                            (ev.modifiers.control, ev.modifiers.shift);
                                        card.update(cx, |deck, cx| {
                                            deck.click_clip(&path, ctrl, shift, cx);
                                        });
                                    }
                                })
                                .on_click({
                                    let card = card.clone();
                                    move |ev: &ClickEvent, _window, cx| {
                                        // Double-click opens, single click
                                        // only selects — the file-manager
                                        // convention, and it keeps an
                                        // accidental click from launching a
                                        // video player over the deck.
                                        if ev.click_count() < 2 {
                                            return;
                                        }
                                        let path = path_for_open.clone();
                                        card.update(cx, |deck, cx| {
                                            deck.open_clip(&path, cx);
                                        });
                                    }
                                });

                            if selected {
                                row = row.bg(palette.panel_2);
                            }

                            // Thumbnail or placeholder. A missing texture is
                            // kicked off here and repaints when it resolves.
                            if let Some(image) = thumb {
                                row = row.child(
                                    img(ImageSource::Render(image))
                                        .object_fit(ObjectFit::Cover)
                                        .w(px(THUMB_W))
                                        .h(px(48.0))
                                        .rounded_sm(),
                                );
                            } else {
                                // Deferred, not inline: mutating the deck while
                                // it is being rendered would re-enter its
                                // borrow. `defer` runs after this effect cycle.
                                {
                                    let card = card.clone();
                                    let path = path.clone();
                                    cx.defer(move |cx| {
                                        card.update(cx, |deck, cx| {
                                            deck.ensure_thumbnail(&path, cx);
                                        });
                                    });
                                }
                                row = row.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .w(px(THUMB_W))
                                        .h(px(48.0))
                                        .rounded_sm()
                                        .bg(palette.panel_2)
                                        .child(
                                            div()
                                                .font_family(crate::ui::theme::MONO)
                                                .text_xs()
                                                .text_color(palette.muted)
                                                .child("…"),
                                        ),
                                );
                            }

                            row.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .truncate()
                                            .child(clip.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .items_center()
                                            .gap_2()
                                            .font_family(crate::ui::theme::MONO)
                                            .text_xs()
                                            .text_color(palette.muted)
                                            .child(crate::ui::format::duration(
                                                clip.duration_seconds,
                                            ))
                                            .child(crate::ui::format::bytes(clip.size_bytes))
                                            .child(crate::ui::format::ago(
                                                clip.created_unix_ms,
                                                now_ms(),
                                            ))
                                            .children(clip.game.clone().map(|game| {
                                                div()
                                                    .px_1()
                                                    .rounded_sm()
                                                    .bg(palette.panel)
                                                    .text_color(palette.cyan)
                                                    .child(game)
                                            }))
                                            .children(clip.tags.first().cloned().map(|tag| {
                                                div()
                                                    .px_1()
                                                    .rounded_sm()
                                                    .bg(palette.panel)
                                                    .text_color(palette.magenta)
                                                    .child(tag)
                                            })),
                                    ),
                            )
                            .child({
                                let card = card.clone();
                                Button::new(("star", ix))
                                    .ghost()
                                    .compact()
                                    .label(if favorite { "★" } else { "☆" })
                                    .tooltip(if favorite {
                                        "Remove from favourites"
                                    } else {
                                        "Add to favourites"
                                    })
                                    .on_click(move |_ev: &ClickEvent, _window, cx| {
                                        let path = path_for_star.clone();
                                        card.update(cx, |deck, cx| {
                                            deck.toggle_favorite(&path, cx);
                                        });
                                    })
                            })
                            .into_any_element()
                        })
                        .collect()
                },
            )
            .track_scroll(&self.library_scroll)
            .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_lg()
            .bg(palette.panel)
            .border_1()
            .border_color(palette.line)
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search).w_full()),
                    )
                    .child(toggles),
            )
            .child(game_chips)
            .child(sort_row)
            .child(div().h(px(ROW_H * 7.0)).w_full().child(list))
    }

    /// One filter/time-range chip.
    fn filter_chip(
        &self,
        palette: Palette,
        label: &str,
        active: bool,
        cx: &mut Context<Self>,
        key: String,
    ) -> Stateful<Div> {
        let _ = cx;
        let all = key == "__all";
        let mut chip = div()
            .id(key)
            .px_2()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .font_family(crate::ui::theme::MONO)
            .text_xs()
            .text_color(if active { palette.cyan } else { palette.muted })
            .child(label.to_string());
        if active {
            chip = chip.bg(palette.panel_2);
        }
        if all {
            chip = chip.on_click(cx.listener(|deck, _ev, _window, cx| {
                deck.set_game_filter(gallery::ALL_GAMES.to_string(), cx);
            }));
        }
        chip
    }

    /// Kicks off a background decode for one visible row. Guarded by
    /// `thumbs_pending` so a repaint cannot queue the same work twice.
    pub(crate) fn ensure_thumbnail(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.thumbs.get_cached(path).is_some() {
            return;
        }
        if !self.thumbs_pending.insert(path.to_string()) {
            return;
        }
        let path_owned = path.to_string();
        cx.spawn(async move |deck, cx| {
            let for_decode = path_owned.clone();
            let image = cx
                .background_spawn(async move {
                    let p = std::path::PathBuf::from(&for_decode);
                    let mut cache = crate::ui::thumbs::ThumbnailCache::new();
                    cache.get_or_load(&p, 1.0)
                })
                .await;
            deck.update(cx, |deck, cx| {
                deck.thumbs_pending.remove(&path_owned);
                if let Some(image) = image {
                    deck.thumbs.insert_shared(path_owned, image);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn empty_state(palette: Palette, library_empty: bool) -> impl IntoElement {
    let (title, body) = if library_empty {
        (
            "No clips yet",
            "Arm the buffer and press SAVE — or hit your save hotkey mid-game.",
        )
    } else {
        (
            "Nothing matches these filters",
            "Clear the search or pick a different game to see your clips again.",
        )
    };
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .h(px(ROW_H * 3.0))
        .child(
            div()
                .text_sm()
                .text_color(palette.text)
                .child(title),
        )
        .child(
            div()
                .font_family(crate::ui::theme::MONO)
                .text_xs()
                .text_color(palette.muted)
                .child(body),
        )
}

/// Display name for a game folder: capitalize a little and keep the rest.
fn game_label(game: &str) -> String {
    if game.is_empty() {
        "Unorganised".to_string()
    } else {
        game.to_string()
    }
}

fn sort_label(key: SortKey) -> &'static str {
    match key {
        SortKey::Newest => "Newest",
        SortKey::Oldest => "Oldest",
        SortKey::Largest => "Largest",
        SortKey::Smallest => "Smallest",
        SortKey::Longest => "Longest",
        SortKey::Shortest => "Shortest",
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

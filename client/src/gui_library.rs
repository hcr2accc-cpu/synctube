//! The "Za tebe" tab (own playlists and recommendations) and the windows
//! that go with it. A child of `gui`, so it can use its widgets.

use super::*;
use crate::engine::{SaveItem, SaveTo};
use crate::library::{Playlist, Song};

/// Narrowest playlist tile; as many as fit go in a row.
const TILE_W: f32 = 210.0;
const TILE_GAP: f32 = 8.0;
/// Rows of the playlist picker shown before it scrolls.
const PICKER_ROWS: f32 = 6.5;
const PICKER_ROW_H: f32 = 44.0;

/// What the tab and its windows are showing.
#[derive(Default)]
pub(super) struct LibraryUi {
    /// The last pass that drew "Za tebe", and whether there was anything to
    /// recommend by.
    shown: Option<(u64, bool)>,
    /// "Za tebe" is selected instead of the queue.
    pub open: bool,
    /// The playlist being looked at.
    playlist: Option<Uuid>,
    picker: Option<Picker>,
    naming: Option<Naming>,
    confirm: Option<Confirm>,
    /// Songs queued from this tab; shown with a check.
    added: HashSet<String>,
}

impl LibraryUi {
    /// One of its windows is open over the room.
    pub fn modal_open(&self) -> bool {
        self.picker.is_some() || self.naming.is_some() || self.confirm.is_some()
    }

    pub fn pick(&mut self, item: SaveItem, title: &str) {
        self.picker = Some(Picker {
            item,
            title: title.to_string(),
            new_name: String::new(),
            focused: false,
        });
    }

    pub fn pick_song(&mut self, song: Song) {
        let title = song.title.clone();
        self.pick(SaveItem::Songs(vec![song]), &title);
    }
}

/// Choosing the playlist to save something to.
struct Picker {
    item: SaveItem,
    title: String,
    new_name: String,
    focused: bool,
}

/// Naming a new playlist, or renaming one.
struct Naming {
    playlist: Option<Uuid>,
    text: String,
    focused: bool,
}

#[derive(Clone, Copy)]
enum Confirm {
    DeletePlaylist(Uuid),
    ClearHistory,
}

/// A search result as a song to save. `None` for a playlist.
pub(super) fn hit_song(h: &SearchHit) -> Option<Song> {
    (!h.playlist).then(|| Song {
        youtube_id: h.youtube_id.clone(),
        title: h.title.clone(),
        duration_ms: h.duration_ms,
        channel: h.channel.clone(),
    })
}

fn song_hit(s: &Song) -> SearchHit {
    SearchHit {
        youtube_id: s.youtube_id.clone(),
        title: s.title.clone(),
        channel: s.channel.clone(),
        duration_ms: s.duration_ms,
        playlist: false,
        thumb_id: Some(s.youtube_id.clone()),
        count: None,
    }
}

impl App {
    /// "Red pesama" and "Za tebe", as the heading of the list.
    pub(super) fn room_tabs(&mut self, ui: &mut egui::Ui) {
        for (library, text) in [(false, s::QUEUE), (true, s::TAB_FOR_YOU)] {
            let on = self.lib.open == library;
            let g = |color| {
                ui.painter()
                    .layout_no_wrap(text.to_string(), bold_font(15.5), color)
            };
            let size = g(theme::TEXT).size();
            let (rect, resp) = ui.allocate_exact_size(vec2(size.x, 30.0), Sense::click());
            let color = if on || resp.hovered() {
                theme::TEXT
            } else {
                theme::TEXT_FAINT
            };
            let g = ui
                .painter()
                .layout_no_wrap(text.to_string(), bold_font(15.5), color);
            ui.painter()
                .galley(pos2(rect.left(), rect.center().y - size.y / 2.0), g, color);
            if on {
                ui.painter().line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(2.0, theme::accent()),
                );
            }
            ui.add_space(8.0);
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() && !on {
                self.lib.open = library;
                self.lib.added.clear();
            }
        }
    }

    pub(super) fn library_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let library = view.library.clone();
        let open = self
            .lib
            .playlist
            .and_then(|id| library.playlists.iter().find(|p| p.id == id));
        match open {
            Some(p) => self.playlist_ui(ui, p, room),
            None => {
                self.lib.playlist = None;
                self.for_you_ui(ui, view, room);
            }
        }
    }

    fn for_you_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let library = view.library.clone();
        // Ask for recommendations when the tab comes into view, and when the
        // first song to go by turns up meanwhile. Fresh ones are kept.
        let pass = ui.ctx().cumulative_pass_nr();
        let source = library.has_history
            || !room.queue.is_empty()
            || library.playlists.iter().any(|p| !p.tracks.is_empty());
        let last = self.lib.shown.replace((pass, source));
        if last.is_none_or(|(p, had)| pass > p + 1 || (source && !had)) {
            self.send(Command::ShowRecs);
        }
        ui.horizontal(|ui| {
            self.room_tabs(ui);
            if library.importing > 0 {
                ui.add(egui::Spinner::new().size(14.0).color(theme::accent()));
                ui.label(
                    RichText::new(s::IMPORTING)
                        .size(12.0)
                        .color(theme::TEXT_DIM),
                );
            }
        });
        ui.add_space(6.0);
        let in_queue: HashSet<&str> = room.queue.iter().map(|t| t.youtube_id.as_str()).collect();
        egui::ScrollArea::vertical()
            .id_salt("for_you_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    section_label(ui, s::MY_PLAYLISTS);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(s::NEW_PLAYLIST).min_size(vec2(0.0, 28.0)))
                            .clicked()
                        {
                            self.lib.naming = Some(Naming {
                                playlist: None,
                                text: String::new(),
                                focused: false,
                            });
                        }
                    });
                });
                ui.add_space(2.0);
                if library.playlists.is_empty() {
                    ui.label(
                        RichText::new(s::NO_PLAYLISTS)
                            .size(13.0)
                            .color(theme::TEXT_DIM),
                    );
                } else {
                    self.tiles_ui(ui, &library.playlists);
                }
                ui.add_space(16.0);

                ui.horizontal(|ui| {
                    section_label(ui, s::RECOMMENDED);
                    if library.recs_loading {
                        ui.add(egui::Spinner::new().size(14.0).color(theme::accent()));
                        ui.label(
                            RichText::new(s::RECS_LOADING)
                                .size(12.0)
                                .color(theme::TEXT_DIM),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if library.has_history
                            && icons::ghost(ui, Icon::Trash, 28.0, true)
                                .on_hover_text(s::CLEAR_HISTORY)
                                .clicked()
                        {
                            self.lib.confirm = Some(Confirm::ClearHistory);
                        }
                        let refresh = egui::Button::new(s::REFRESH).min_size(vec2(0.0, 28.0));
                        if ui.add_enabled(!library.recs_loading, refresh).clicked() {
                            self.send(Command::RefreshRecs);
                        }
                    });
                });
                ui.add_space(2.0);
                if let Some(e) = &library.recs_error {
                    ui.label(RichText::new(e).size(13.0).color(theme::WARN));
                }
                if library.recs.is_empty() {
                    if !library.recs_loading && library.recs_error.is_none() {
                        empty_state(ui, s::RECS_EMPTY, s::RECS_EMPTY_HINT);
                    }
                    return;
                }
                ui.spacing_mut().item_spacing.y = ROW_GAP;
                for song in &library.recs {
                    let added = in_queue.contains(song.youtube_id.as_str())
                        || self.lib.added.contains(&song.youtube_id);
                    if self.search_row(ui, &song_hit(song), added) {
                        self.lib.added.insert(song.youtube_id.clone());
                    }
                }
            });
    }

    /// The user's playlists, a few per row.
    fn tiles_ui(&mut self, ui: &mut egui::Ui, playlists: &[Playlist]) {
        let width = ui.available_width();
        let columns = ((width + TILE_GAP) / (TILE_W + TILE_GAP)).floor().max(1.0);
        let w = (width - TILE_GAP * (columns - 1.0)) / columns;
        ui.spacing_mut().item_spacing = vec2(TILE_GAP, TILE_GAP);
        for row in playlists.chunks(columns as usize) {
            ui.horizontal(|ui| {
                for p in row {
                    if self.tile(ui, p, w).clicked() {
                        self.lib.playlist = Some(p.id);
                    }
                }
            });
        }
    }

    fn tile(&mut self, ui: &mut egui::Ui, p: &Playlist, w: f32) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
        let painter = ui.painter().clone();
        let fill = if resp.hovered() {
            theme::CARD_HOVER
        } else {
            theme::CARD
        };
        painter.rect_filled(rect, 8.0, fill);
        let thumb = Rect::from_min_size(
            pos2(rect.left() + 8.0, rect.center().y - 18.0),
            vec2(64.0, 36.0),
        );
        let tex = p
            .tracks
            .first()
            .and_then(|t| self.thumbs.get(&t.youtube_id));
        paint_thumb(ui, thumb, tex, 5);
        let left = thumb.right() + 10.0;
        let max_w = (rect.right() - 10.0 - left).max(30.0);
        let name = one_line(ui, &p.name, bold_font(14.0), theme::TEXT, max_w);
        painter.galley(
            pos2(left, rect.center().y - name.size().y - 1.0),
            name,
            theme::TEXT,
        );
        painter.text(
            pos2(left, rect.center().y + 2.0),
            Align2::LEFT_TOP,
            s::songs(p.tracks.len()),
            font(12.0),
            theme::TEXT_DIM,
        );
        resp.on_hover_cursor(CursorIcon::PointingHand)
    }

    fn playlist_ui(&mut self, ui: &mut egui::Ui, p: &Playlist, room: &RoomState) {
        let mut back = false;
        ui.horizontal(|ui| {
            back = ui
                .add(egui::Button::new(s::BACK).min_size(vec2(0.0, 28.0)))
                .clicked();
            ui.add_space(2.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if icons::ghost(ui, Icon::Trash, 28.0, true)
                    .on_hover_text(s::DELETE_PLAYLIST)
                    .clicked()
                {
                    self.lib.confirm = Some(Confirm::DeletePlaylist(p.id));
                }
                if ui
                    .add(egui::Button::new(s::RENAME).min_size(vec2(0.0, 28.0)))
                    .clicked()
                {
                    self.lib.naming = Some(Naming {
                        playlist: Some(p.id),
                        text: p.name.clone(),
                        focused: false,
                    });
                }
                if primary_button(
                    ui,
                    Some(Icon::Plus),
                    s::ADD_ALL,
                    vec2(150.0, 28.0),
                    !p.tracks.is_empty(),
                )
                .clicked()
                {
                    self.send(Command::QueuePlaylist(p.id));
                    self.lib
                        .added
                        .extend(p.tracks.iter().map(|t| t.youtube_id.clone()));
                }
                ui.add_space(4.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let total: u64 = p.tracks.iter().filter_map(|t| t.duration_ms).sum();
                    let mut sub = s::songs(p.tracks.len());
                    if total > 0 {
                        sub.push_str(" · ");
                        sub.push_str(&fmt_time(total));
                    }
                    // The name gets the room the count leaves.
                    let sub = RichText::new(sub).size(12.0).color(theme::TEXT_FAINT);
                    let name_w = (ui.available_width() - 130.0).max(40.0);
                    let g = one_line(ui, &p.name, bold_font(15.5), theme::TEXT, name_w);
                    ui.label(g).on_hover_text(&p.name);
                    ui.add(egui::Label::new(sub).truncate());
                });
            });
        });
        ui.add_space(4.0);
        let escape = !self.modal_open()
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.key_pressed(Key::Escape));
        if back || escape {
            self.lib.playlist = None;
            return;
        }
        if p.tracks.is_empty() {
            empty_state(ui, s::PLAYLIST_EMPTY, s::PLAYLIST_EMPTY_HINT);
            return;
        }
        let in_queue: HashSet<&str> = room.queue.iter().map(|t| t.youtube_id.as_str()).collect();
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        egui::ScrollArea::vertical()
            .id_salt(("playlist_scroll", p.id))
            .auto_shrink([false, false])
            .show_rows(ui, ROW_H, p.tracks.len(), |ui, range| {
                for i in range {
                    let id = p.tracks[i].youtube_id.as_str();
                    let added = in_queue.contains(id) || self.lib.added.contains(id);
                    self.saved_row(ui, p, i, added);
                }
            });
    }

    /// A song of one of the user's playlists.
    fn saved_row(&mut self, ui: &mut egui::Ui, p: &Playlist, i: usize, added: bool) {
        let song = &p.tracks[i];
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter().clone();
        if hovered {
            painter.rect_filled(rect, 8.0, theme::CARD);
        }
        let num = Rect::from_min_size(rect.min, vec2(40.0, ROW_H));
        painter.text(
            num.center(),
            Align2::CENTER_CENTER,
            (i + 1).to_string(),
            font(12.5),
            theme::TEXT_FAINT,
        );
        let thumb =
            Rect::from_min_size(pos2(num.right(), rect.center().y - 18.0), vec2(64.0, 36.0));
        let tex = self.thumbs.get(&song.youtube_id);
        paint_thumb(ui, thumb, tex, 5);

        let btn =
            Rect::from_center_size(pos2(rect.right() - 24.0, rect.center().y), vec2(30.0, 30.0));
        let add = add_circle(ui, btn, ("saved_add", p.id, &song.youtube_id), added, false)
            || (!added && resp.double_clicked());

        let actions_w = if hovered { 3.0 * 28.0 } else { 0.0 };
        let dur_right = btn.left() - 14.0 - actions_w;
        let left = thumb.right() + 12.0;
        let max_w = (dur_right - 56.0 - left).max(40.0);
        let title = one_line(ui, &song.title, font(14.0), theme::TEXT, max_w);
        match &song.channel {
            Some(ch) => {
                painter.galley(
                    pos2(left, rect.center().y - title.size().y - 1.0),
                    title,
                    theme::TEXT,
                );
                let g = one_line(ui, ch, font(12.0), theme::TEXT_DIM, max_w);
                painter.galley(pos2(left, rect.center().y + 2.0), g, theme::TEXT_DIM);
            }
            None => painter.galley(
                pos2(left, rect.center().y - title.size().y / 2.0),
                title,
                theme::TEXT,
            ),
        };
        if let Some(d) = song.duration_ms {
            painter.text(
                pos2(dur_right, rect.center().y),
                Align2::RIGHT_CENTER,
                fmt_time(d),
                font(12.5),
                theme::TEXT_FAINT,
            );
        }
        if hovered {
            let mut ui = ui.new_child(
                UiBuilder::new()
                    .id_salt(("saved_actions", p.id, &song.youtube_id))
                    .max_rect(Rect::from_min_max(
                        pos2(btn.left() - 8.0 - actions_w, rect.top()),
                        pos2(btn.left() - 8.0, rect.bottom()),
                    ))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            ui.spacing_mut().item_spacing.x = 2.0;
            let id = || song.youtube_id.clone();
            if icons::ghost(&mut ui, Icon::Remove, 26.0, true)
                .on_hover_text(s::REMOVE_FROM_PLAYLIST)
                .clicked()
            {
                self.send(Command::Unsave(p.id, id()));
            }
            if icons::ghost(&mut ui, Icon::Down, 26.0, i + 1 < p.tracks.len())
                .on_hover_text(s::MOVE_DOWN)
                .clicked()
            {
                self.send(Command::MoveSaved(p.id, id(), i + 1));
            }
            if icons::ghost(&mut ui, Icon::Up, 26.0, i > 0)
                .on_hover_text(s::MOVE_UP)
                .clicked()
            {
                self.send(Command::MoveSaved(p.id, id(), i - 1));
            }
        }
        if add {
            self.send(Command::Add(shared::youtube::watch_url(&song.youtube_id)));
            self.lib.added.insert(song.youtube_id.clone());
        }
    }

    // ----- windows ----------------------------------------------------------------

    pub(super) fn library_modals(&mut self, ctx: &egui::Context, view: &View) {
        if self.lib.picker.is_some() {
            self.picker_ui(ctx, view);
        }
        if self.lib.naming.is_some() {
            self.naming_ui(ctx);
        }
        if let Some(c) = self.lib.confirm {
            self.library_confirm_ui(ctx, view, c);
        }
    }

    fn picker_ui(&mut self, ctx: &egui::Context, view: &View) {
        let Some(mut picker) = self.lib.picker.take() else {
            return;
        };
        let mut chosen: Option<SaveTo> = None;
        let mut close = false;
        let resp = modal(ctx, "save_picker").show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 80.0).clamp(220.0, 360.0));
            modal_title(ui, Icon::ListAdd, s::SAVE_TO_PLAYLIST);
            ui.add(
                egui::Label::new(
                    RichText::new(&picker.title)
                        .size(13.0)
                        .color(theme::TEXT_DIM),
                )
                .truncate(),
            );
            ui.add_space(10.0);
            let playlists = &view.library.playlists;
            if !playlists.is_empty() {
                egui::ScrollArea::vertical()
                    .id_salt("picker_scroll")
                    .max_height(PICKER_ROWS * PICKER_ROW_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for p in playlists {
                            let has = match &picker.item {
                                SaveItem::Songs(songs) => {
                                    songs.iter().all(|t| p.contains(&t.youtube_id))
                                }
                                SaveItem::YoutubePlaylist(_) => false,
                            };
                            if picker_row(ui, p, has) {
                                chosen = Some(SaveTo::Existing(p.id));
                            }
                        }
                    });
                ui.add_space(10.0);
            }
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut picker.new_name)
                        .hint_text(s::NEW_PLAYLIST_HINT)
                        .font(font(14.0))
                        .margin(vec2(10.0, 8.0))
                        .char_limit(crate::library::MAX_NAME_CHARS)
                        .desired_width((ui.available_width() - 108.0).max(80.0)),
                );
                // Nothing to pick from yet: the name is the only thing to fill in.
                if !std::mem::replace(&mut picker.focused, true) && playlists.is_empty() {
                    r.request_focus();
                }
                let named = !picker.new_name.trim().is_empty();
                let enter = enter_pressed(ui, &r);
                let size = vec2(98.0, r.rect.height().max(34.0));
                let clicked = primary_button(ui, None, s::CREATE, size, named).clicked();
                if named && (clicked || enter) {
                    chosen = Some(SaveTo::New(picker.new_name.clone()));
                }
            });
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = ui
                    .add(egui::Button::new(s::CLOSE).min_size(vec2(90.0, 32.0)))
                    .clicked();
            });
        });
        if let Some(to) = chosen {
            self.send(Command::Save {
                to,
                item: picker.item,
            });
        } else if !close && !resp.should_close() {
            self.lib.picker = Some(picker);
        }
    }

    fn naming_ui(&mut self, ctx: &egui::Context) {
        let Some(mut n) = self.lib.naming.take() else {
            return;
        };
        let (mut yes, mut no) = (false, false);
        let resp = modal(ctx, "playlist_name").show(ctx, |ui| {
            ui.set_width((ctx.content_rect().width() - 80.0).clamp(220.0, 340.0));
            let (title, button) = match n.playlist {
                Some(_) => (s::RENAME, s::SAVE),
                None => (s::NEW_PLAYLIST, s::CREATE),
            };
            modal_title(ui, Icon::ListAdd, title);
            ui.add_space(8.0);
            let r = ui.add(
                egui::TextEdit::singleline(&mut n.text)
                    .hint_text(s::NEW_PLAYLIST_HINT)
                    .font(font(14.5))
                    .margin(vec2(10.0, 8.0))
                    .char_limit(crate::library::MAX_NAME_CHARS)
                    .desired_width(f32::INFINITY),
            );
            if !std::mem::replace(&mut n.focused, true) {
                r.request_focus();
            }
            let named = !n.text.trim().is_empty();
            let enter = enter_pressed(ui, &r);
            ui.add_space(14.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let clicked = primary_button(ui, None, button, vec2(110.0, 34.0), named).clicked();
                yes = named && (clicked || enter);
                no = ui
                    .add(egui::Button::new(s::CANCEL).min_size(vec2(90.0, 34.0)))
                    .clicked();
            });
        });
        if yes {
            self.send(match n.playlist {
                Some(id) => Command::RenamePlaylist(id, n.text),
                None => Command::CreatePlaylist(n.text),
            });
        } else if !no && !resp.should_close() {
            self.lib.naming = Some(n);
        }
    }

    fn library_confirm_ui(&mut self, ctx: &egui::Context, view: &View, what: Confirm) {
        let (title, text, button) = match what {
            Confirm::DeletePlaylist(id) => {
                let Some(p) = view.library.playlists.iter().find(|p| p.id == id) else {
                    self.lib.confirm = None;
                    return;
                };
                (
                    s::delete_playlist_title(&p.name),
                    s::delete_playlist_explain(p.tracks.len()),
                    s::DELETE,
                )
            }
            Confirm::ClearHistory => (
                s::CLEAR_HISTORY_TITLE.to_string(),
                s::CLEAR_HISTORY_EXPLAIN.to_string(),
                s::DELETE,
            ),
        };
        match confirm_modal(ctx, "library_confirm", &title, &text, button) {
            Some(true) => {
                self.lib.confirm = None;
                self.send(match what {
                    Confirm::DeletePlaylist(id) => Command::DeletePlaylist(id),
                    Confirm::ClearHistory => Command::ClearHistory,
                });
            }
            Some(false) => self.lib.confirm = None,
            None => {}
        }
    }
}

fn picker_row(ui: &mut egui::Ui, p: &Playlist, has: bool) -> bool {
    let sense = if has { Sense::hover() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), PICKER_ROW_H), sense);
    let painter = ui.painter();
    if resp.hovered() && !has {
        painter.rect_filled(rect, 8.0, theme::CARD_HOVER);
    }
    let color = if has { theme::TEXT_DIM } else { theme::TEXT };
    let left = rect.left() + 10.0;
    let g = one_line(ui, &p.name, font(14.0), color, rect.width() - 60.0);
    painter.galley(pos2(left, rect.center().y - g.size().y), g, color);
    let mut sub = s::songs(p.tracks.len());
    if has {
        sub = format!("{sub} · {}", s::ALREADY_SAVED);
        let r =
            Rect::from_center_size(pos2(rect.right() - 20.0, rect.center().y), vec2(13.0, 13.0));
        icons::paint(painter, Icon::Check, r, theme::accent());
    }
    painter.text(
        pos2(left, rect.center().y + 1.0),
        Align2::LEFT_TOP,
        sub,
        font(12.0),
        theme::TEXT_FAINT,
    );
    !has && resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

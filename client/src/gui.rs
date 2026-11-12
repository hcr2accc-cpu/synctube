//! egui front-end: dependency download, connect screen and room screen.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, Event, FontId, Frame, Id,
    Key, LayerId, Layout, Margin, Order, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder,
};
use shared::{MemberSync, PlayStatus, RoomState};
use uuid::Uuid;

use crate::config::Config;
use crate::deps::{self, DepPhase};
use crate::engine::{
    self, Command, ConnState, EngineConfig, EngineHandle, PendingAdd, Phase, View,
};
use crate::icons::{self, Icon};
use crate::paths::Tools;
use crate::strings as s;
use crate::theme::{self, bold_font, font};
use crate::thumbs::Thumbs;
use crate::ytdlp::SearchHit;

const TOAST_SECS: f32 = 5.0;
const TOAST_FADE: f32 = 0.35;
const ROW_H: f32 = 52.0;
const ROW_GAP: f32 = 2.0;
const SIDEBAR_W: f32 = 236.0;
/// Keep showing a seek target until the player catches up (avoids the
/// progress bar jumping back for a moment).
const SEEK_HOLD: Duration = Duration::from_millis(1500);
/// Height of the full-screen video controls.
const FULL_BAR_H: f32 = 56.0;
/// Full-screen controls hide after this long without mouse movement.
const CONTROLS_HIDE: Duration = Duration::from_millis(2500);
/// The accent takes this long (seconds) to move to a new song's colour.
const ACCENT_FADE: f32 = 0.6;

enum Screen {
    Deps(deps::Shared),
    Connect,
    Room,
}

enum Scroll {
    Index(usize),
    End,
}

/// A queue row being dragged to a new place.
struct QueueDrag {
    id: Uuid,
    /// Pointer offset from the row's top, so the floating row doesn't jump.
    grab_dy: f32,
    /// Insertion slot (0..=len) under the pointer.
    slot: usize,
}

pub struct App {
    engine: Option<EngineHandle>,
    config: Config,
    screen: Screen,
    /// Connect pressed; waiting for the server to accept us.
    connecting: bool,
    connect_error: Option<String>,
    link: String,
    focus_link: bool,
    filter: String,
    volume: f64,
    muted_from: Option<f64>,
    thumbs: Thumbs,
    last_current: Option<Uuid>,
    scroll: Option<Scroll>,
    drag: Option<QueueDrag>,
    /// The YouTube search results replace the queue while open.
    search_open: bool,
    /// Videos added from the current search (shown with a check).
    search_added: HashSet<String>,
    seek_drag: Option<f32>,
    seek_hold: Option<(u64, Instant)>,
    bar_width: f32,
    notify: Arc<dyn Fn() + Send + Sync>,
    /// Search finds playlists instead of songs.
    search_playlists: bool,
    settings_open: bool,
    /// Last `View::video_closed` seen.
    video_closed_seen: u64,
    /// Native area inside the window that mpv draws the video into.
    video_host: Option<crate::video_host::VideoHost>,
    /// Where the video goes this frame (set while laying out).
    video_rect: Option<Rect>,
    /// Video fills the whole (full-screen) window.
    video_full: bool,
    /// Bottom part of the video covered by the full-screen controls.
    video_cut: f32,
    /// Last mouse/key activity in full screen (shows the controls).
    controls_at: Instant,
    /// Pointer position last frame in full screen.
    full_pointer: Option<egui::Pos2>,
    /// The accent fades from one song's colour to the next.
    accent_from: Color32,
    accent_to: Color32,
    accent_at: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, fake_audio: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx);
        let thumbs = Thumbs::new(ctx.clone());
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let config = Config::load();
        let tools = Tools::discover();
        let video_host = crate::video_host::VideoHost::new(cc);
        let screen = if !fake_audio && deps::needed(&tools) {
            Screen::Deps(deps::start(tools.clone(), notify.clone()))
        } else {
            Screen::Connect
        };
        let engine = engine::start(
            EngineConfig {
                fake_audio,
                fake_rate: 1.0,
                tools,
                volume: config.volume,
                normalize: config.normalize,
                auto_update: true,
                eq: config.eq.clone().clamped(),
                video: config.video,
                video_wid: video_host.as_ref().map(|h| h.wid()),
            },
            notify.clone(),
        );
        Self {
            engine: Some(engine),
            volume: config.volume,
            search_playlists: config.search_playlists,
            settings_open: false,
            video_closed_seen: 0,
            video_host,
            video_rect: None,
            video_full: false,
            video_cut: 0.0,
            controls_at: Instant::now(),
            full_pointer: None,
            accent_from: theme::ACCENT,
            accent_to: theme::ACCENT,
            accent_at: Instant::now(),
            config,
            screen,
            connecting: false,
            connect_error: None,
            link: String::new(),
            focus_link: false,
            filter: String::new(),
            drag: None,
            search_open: false,
            search_added: HashSet::new(),
            muted_from: None,
            thumbs,
            last_current: None,
            scroll: None,
            seek_drag: None,
            seek_hold: None,
            bar_width: 400.0,
            notify,
        }
    }

    fn send(&self, c: Command) {
        if let Some(e) = &self.engine {
            e.send(c);
        }
    }

    fn view(&self) -> View {
        self.engine.as_ref().map(|e| e.view()).unwrap_or_default()
    }

    /// Moves the accent toward the colour of the playing song's thumbnail
    /// (the brand accent when nothing plays or the picture is grey).
    fn update_accent(&mut self, ctx: &egui::Context, view: &View) {
        let track = match self.screen {
            Screen::Room => view.room.as_ref().and_then(|r| r.current_track()),
            _ => None,
        };
        let target = match track {
            None => Some(theme::ACCENT),
            // While the picture loads, keep the colour we have instead of
            // flashing back to the brand accent between songs.
            Some(t) => self
                .thumbs
                .tint(&t.youtube_id)
                .map(|tint| tint.map_or(theme::ACCENT, theme::accent_from)),
        };
        let shown = theme::accent();
        if let Some(target) = target {
            if target != self.accent_to {
                self.accent_from = shown;
                self.accent_to = target;
                self.accent_at = Instant::now();
            }
        }
        let t = (self.accent_at.elapsed().as_secs_f32() / ACCENT_FADE).min(1.0);
        let eased = t * t * (3.0 - 2.0 * t);
        let c = theme::mix(self.accent_from, self.accent_to, eased);
        if c != shown {
            theme::set_accent(c);
            ctx.style_mut_of(egui::Theme::Dark, |s| theme::apply_accent(&mut s.visuals));
        }
        if t < 1.0 {
            ctx.request_repaint();
        }
    }

    fn set_volume(&mut self, v: f64) {
        self.volume = v;
        self.send(Command::SetVolume(v));
        self.config.volume = v;
        self.config.save();
    }

    // ----- dependency screen ---------------------------------------------------

    fn deps_ui(&mut self, ui: &mut egui::Ui, view: &View, shared: deps::Shared) {
        let progress = shared.lock().unwrap().clone();
        if let Some(Ok(tools)) = &progress.finished {
            self.send(Command::SetTools(tools.clone()));
            self.screen = Screen::Connect;
            return;
        }
        let mut retry = false;
        background_panel().show(ui, |ui| {
            footer(ui, view);
            centered_card(ui, "deps_card", 460.0, |ui| {
                brand_header(ui, s::DEPS_HEADING, s::DEPS_EXPLAIN);
                ui.add_space(14.0);
                for item in &progress.items {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(item.name).font(bold_font(14.0)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (text, color) = match &item.phase {
                                DepPhase::Waiting => {
                                    (s::DEP_WAITING.to_string(), theme::TEXT_FAINT)
                                }
                                DepPhase::Downloading { done, total } => {
                                    let mb = *done as f32 / 1_048_576.0;
                                    let t = match total {
                                        Some(t) if *t > 0 => {
                                            format!("{mb:.1} / {:.1} MB", *t as f32 / 1_048_576.0)
                                        }
                                        _ => format!("{mb:.1} MB"),
                                    };
                                    (t, theme::TEXT_DIM)
                                }
                                DepPhase::Extracting => (s::DEP_EXTRACTING.into(), theme::TEXT_DIM),
                                DepPhase::Verifying => (s::DEP_VERIFYING.into(), theme::TEXT_DIM),
                                DepPhase::Done => (s::DEP_DONE.into(), theme::GOOD),
                                DepPhase::Failed(e) => (e.clone(), theme::BAD),
                            };
                            ui.label(RichText::new(text).size(12.5).color(color));
                            match &item.phase {
                                DepPhase::Done => {
                                    icons::label(ui, Icon::Check, 14.0, theme::GOOD);
                                }
                                DepPhase::Extracting | DepPhase::Verifying => {
                                    ui.add(egui::Spinner::new().size(13.0).color(theme::accent()));
                                }
                                _ => {}
                            }
                        });
                    });
                    let frac = match &item.phase {
                        DepPhase::Downloading {
                            done,
                            total: Some(t),
                        } if *t > 0 => *done as f32 / *t as f32,
                        DepPhase::Extracting | DepPhase::Verifying | DepPhase::Done => 1.0,
                        _ => 0.0,
                    };
                    let color = match &item.phase {
                        DepPhase::Done => theme::GOOD,
                        DepPhase::Failed(_) => theme::BAD,
                        _ => theme::accent(),
                    };
                    thin_bar(ui, frac, color);
                    ui.add_space(6.0);
                }
                if let Some(Err(e)) = &progress.finished {
                    ui.add_space(6.0);
                    error_box(ui, &format!("{} {e}", s::DEPS_FAILED));
                    ui.label(
                        RichText::new(s::MISSING_TOOLS_PLAYBACK)
                            .size(12.5)
                            .color(theme::TEXT_DIM),
                    );
                    ui.add_space(4.0);
                    retry =
                        primary_button(ui, None, s::RETRY, vec2(ui.available_width(), 38.0), true)
                            .clicked();
                }
            });
        });
        if retry {
            let tools = Tools::discover();
            self.send(Command::SetTools(tools.clone()));
            self.screen = if deps::needed(&tools) {
                Screen::Deps(deps::start(tools, self.notify.clone()))
            } else {
                Screen::Connect
            };
        }
    }

    // ----- connect screen --------------------------------------------------------

    fn connect_ui(&mut self, ui: &mut egui::Ui, view: &View) {
        if self.connecting {
            match &view.conn {
                _ if view.joined => {
                    self.connecting = false;
                    self.connect_error = None;
                    self.config.remember_current();
                    self.config.save();
                    self.screen = Screen::Room;
                    return;
                }
                ConnState::Rejected(r) | ConnState::Failed(r) => {
                    self.connect_error = Some(r.clone());
                    self.connecting = false;
                }
                _ => {}
            }
        }
        let mut submit = false;
        let mut cancel = false;
        let mut recent_action = None;
        background_panel().show(ui, |ui| {
            footer(ui, view);
            centered_card(ui, "connect_card", 420.0, |ui| {
                brand_header(ui, s::APP_TITLE, s::CONNECT_HEADING);
                ui.add_space(16.0);
                ui.add_enabled_ui(!self.connecting, |ui| {
                    let c = &mut self.config;
                    for (label, value, password, hint) in [
                        (s::ROOM, &mut c.room, false, s::ROOM_HINT),
                        (s::NICKNAME, &mut c.name, false, s::NICK_HINT),
                        (s::PASSWORD, &mut c.password, true, ""),
                    ] {
                        let r = field(ui, label, value, password, hint);
                        if enter_pressed(ui, &r) {
                            submit = true;
                        }
                        ui.add_space(4.0);
                    }
                    egui::CollapsingHeader::new(
                        RichText::new(s::ADVANCED).size(12.5).color(theme::TEXT_DIM),
                    )
                    .id_salt("advanced")
                    .default_open(c.server_url.trim().is_empty())
                    .show(ui, |ui| {
                        let r = field(
                            ui,
                            s::SERVER_URL,
                            &mut c.server_url,
                            false,
                            s::SERVER_URL_HINT,
                        );
                        if enter_pressed(ui, &r) {
                            submit = true;
                        }
                        // Remember the address even before the first connect.
                        if r.lost_focus() {
                            c.save();
                        }
                    });
                });
                ui.add_space(10.0);
                if self.connecting {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(16.0).color(theme::accent()));
                        let text = match &view.conn {
                            ConnState::Connecting { waking: true } => s::WAKING_SERVER.to_string(),
                            ConnState::Reconnecting { retry_in_s } => {
                                s::reconnecting_in(*retry_in_s)
                            }
                            _ => s::CONNECTING.to_string(),
                        };
                        ui.add(egui::Label::new(RichText::new(text).color(theme::TEXT_DIM)).wrap());
                    });
                    ui.add_space(4.0);
                    cancel = ui
                        .add_sized([ui.available_width(), 36.0], egui::Button::new(s::CANCEL))
                        .clicked();
                } else if primary_button(
                    ui,
                    None,
                    s::CONNECT,
                    vec2(ui.available_width(), 40.0),
                    true,
                )
                .clicked()
                {
                    submit = true;
                }
                if let Some(e) = &self.connect_error {
                    ui.add_space(8.0);
                    error_box(ui, e);
                }
                if !self.connecting && !self.config.recent.is_empty() {
                    recent_action = recent_rooms_ui(ui, &self.config.recent);
                }
            });
        });
        if cancel {
            self.send(Command::Leave);
            self.search_open = false;
            self.connecting = false;
        } else if let Some(action) = recent_action {
            match action {
                RecentAction::Join(i) => {
                    if let Some(r) = self.config.recent.get(i).cloned() {
                        self.config.use_recent(&r);
                        self.try_connect();
                    }
                }
                RecentAction::Forget(i) => {
                    if i < self.config.recent.len() {
                        self.config.recent.remove(i);
                        self.config.save();
                    }
                }
            }
        } else if submit && !self.connecting {
            self.try_connect();
        }
    }

    fn try_connect(&mut self) {
        let c = &mut self.config;
        c.server_url = c.server_url.trim().to_string();
        c.room = c.room.trim().to_string();
        c.name = c.name.trim().to_string();
        if c.server_url.is_empty() || c.room.is_empty() || c.name.is_empty() {
            self.connect_error = Some(s::FILL_ALL.into());
            return;
        }
        if !(c.server_url.starts_with("ws://") || c.server_url.starts_with("wss://")) {
            self.connect_error = Some(s::BAD_SERVER_URL.into());
            return;
        }
        self.connect_error = None;
        self.config.save();
        self.connecting = true;
        self.send(Command::Connect {
            url: self.config.server_url.clone(),
            room: self.config.room.clone(),
            password: self.config.password.clone(),
            name: self.config.name.clone(),
        });
    }

    // ----- room screen -------------------------------------------------------------

    fn room_ui(&mut self, ui: &mut egui::Ui, view: &View) {
        match &view.conn {
            ConnState::Rejected(r) | ConnState::Failed(r) => {
                self.connect_error = Some(r.clone());
                self.screen = Screen::Connect;
                return;
            }
            ConnState::Idle => {
                self.screen = Screen::Connect;
                return;
            }
            _ => {}
        }
        let Some(room) = view.room.clone() else {
            // Joined but no state yet (or reconnecting from scratch).
            background_panel().show(ui, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Spinner::new().size(28.0).color(theme::accent()));
                });
            });
            return;
        };

        self.global_keys(ui, &room);

        let show_video = self.config.video && room.current_track().is_some();
        if self.video_full {
            let leave = !show_video || ui.input(|i| i.key_pressed(Key::Escape));
            if leave {
                self.set_video_full(ui.ctx(), false);
            } else {
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(Color32::BLACK))
                    .show(ui, |ui| self.video_full_ui(ui, view, &room));
                return;
            }
        }

        let cur_id = room.current_track().map(|t| t.id);
        if cur_id != self.last_current {
            self.last_current = cur_id;
            if let Some(i) = room.playback.track_index {
                self.scroll = Some(Scroll::Index(i));
            }
        }

        egui::Panel::top("room_top")
            .exact_size(56.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(16, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
                    icons::logo(ui.painter(), r);
                    ui.label(
                        RichText::new(s::APP_TITLE)
                            .font(bold_font(16.5))
                            .color(theme::TEXT),
                    );
                    ui.add_space(6.0);
                    tag(
                        ui,
                        &format!("{} {}", s::ROOM, room.name),
                        theme::CARD,
                        theme::BORDER,
                        theme::TEXT_DIM,
                        None,
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(s::LEAVE).min_size(vec2(0.0, 30.0)))
                            .clicked()
                        {
                            self.send(Command::Leave);
                            self.search_open = false;
                            self.settings_open = false;
                            self.screen = Screen::Connect;
                        }
                        ui.add_space(2.0);
                        if icons::ghost(ui, Icon::Gear, 30.0, true)
                            .on_hover_text(s::SETTINGS)
                            .clicked()
                        {
                            self.settings_open = !self.settings_open;
                        }
                        connection_pill(ui, view);
                    });
                });
            });

        egui::Panel::bottom("room_bottom")
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::symmetric(16, 12)),
            )
            .show(ui, |ui| self.add_bar_ui(ui));

        egui::Panel::right("members")
            .resizable(false)
            .exact_size(SIDEBAR_W)
            .frame(
                Frame::new()
                    .fill(theme::PANEL)
                    .inner_margin(Margin::same(14)),
            )
            .show(ui, |ui| sidebar_ui(ui, view));

        egui::CentralPanel::default()
            .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
            .show(ui, |ui| {
                if show_video {
                    self.video_ui(ui);
                    ui.add_space(14.0);
                }
                self.now_playing_ui(ui, view, &room);
                ui.add_space(14.0);
                if self.search_open {
                    self.search_ui(ui, view, &room);
                } else {
                    self.queue_ui(ui, view, &room);
                }
            });
    }

    /// The video of the song, inside the room screen (16:9, at most half
    /// the height so the queue stays usable).
    fn video_ui(&mut self, ui: &mut egui::Ui) {
        let w = ui.available_width();
        let h = (w * 9.0 / 16.0).min(ui.available_height() * 0.5).max(120.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
        ui.painter().rect(
            rect,
            12.0,
            theme::CARD,
            Stroke::new(1.0, theme::BORDER),
            StrokeKind::Inside,
        );
        let inner = rect.shrink(6.0);
        ui.painter().rect_filled(inner, 6.0, Color32::BLACK);
        if resp.double_clicked() {
            self.set_video_full(ui.ctx(), true);
        }
        resp.on_hover_text(s::VIDEO_FULL_HINT);
        self.video_rect = Some(inner);
    }

    /// Full-screen video with controls that show on mouse movement and
    /// hide while watching.
    fn video_full_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let rect = ui.max_rect();
        self.video_rect = Some(rect);
        // Real mouse movement, a click, scroll or a key shows the controls.
        let (pos, active) = ui.input(|i| {
            let pressed = i.pointer.any_pressed()
                || i.events
                    .iter()
                    .any(|e| matches!(e, Event::MouseWheel { .. }))
                || i.events
                    .iter()
                    .any(|e| matches!(e, Event::Key { pressed: true, .. }));
            (i.pointer.latest_pos(), pressed)
        });
        let moved = match (pos, self.full_pointer) {
            (Some(p), Some(last)) => p.distance(last) > 1.0,
            _ => false,
        };
        self.full_pointer = pos;
        if active || moved {
            self.controls_at = Instant::now();
        }
        let idle = self.controls_at.elapsed();
        let dragging = self.seek_drag.is_some() || ui.ctx().dragged_id().is_some();
        let resp = ui.interact(rect, Id::unique("video_full"), Sense::click());
        if resp.double_clicked() {
            self.set_video_full(ui.ctx(), false);
        }
        if idle >= CONTROLS_HIDE && !dragging {
            ui.ctx().set_cursor_icon(CursorIcon::None);
            return;
        }
        ui.ctx()
            .request_repaint_after(CONTROLS_HIDE.saturating_sub(idle) + Duration::from_millis(30));
        self.video_cut = FULL_BAR_H;
        let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - FULL_BAR_H), rect.max);
        ui.painter()
            .rect_filled(bar, 0.0, Color32::from_rgb(14, 14, 18));

        let track = room.current_track().cloned();
        let mut ui = ui.new_child(
            UiBuilder::new()
                .id_salt("video_full_controls")
                .max_rect(bar.shrink2(vec2(18.0, 2.0)))
                .layout(Layout::top_down(Align::Min)),
        );
        ui.spacing_mut().item_spacing.y = 0.0;
        let (shown, duration) = self.seek_bar(&mut ui, view, track.as_ref(), false);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            self.playback_buttons(ui, room, track.is_some(), true);
            ui.add_space(8.0);
            self.mute_button(ui);
            ui.spacing_mut().slider_width = 80.0;
            self.volume_slider(ui);
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} / {}",
                    fmt_time(shown),
                    duration.map_or("--:--".into(), fmt_time)
                ))
                .size(12.5)
                .color(theme::TEXT_DIM),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::ghost(ui, Icon::ExitFullscreen, 26.0, true)
                    .on_hover_text(s::EXIT_FULLSCREEN)
                    .clicked()
                {
                    self.set_video_full(ui.ctx(), false);
                }
                if let Some(t) = &track {
                    ui.add(
                        egui::Label::new(RichText::new(&t.title).size(12.5).color(theme::TEXT))
                            .truncate(),
                    );
                }
            });
        });
    }

    fn set_video_full(&mut self, ctx: &egui::Context, on: bool) {
        if self.video_full != on {
            self.video_full = on;
            self.controls_at = Instant::now();
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
        }
    }

    /// Ctrl+V anywhere adds a link; Space toggles playback.
    fn global_keys(&mut self, ui: &mut egui::Ui, room: &RoomState) {
        let ctx = ui.ctx().clone();
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pasted = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                Event::Paste(t) => Some(t.trim().to_string()),
                _ => None,
            })
        });
        if let Some(text) = pasted {
            if shared::youtube::parse_youtube_url(&text).is_some() {
                self.send(Command::Add(text));
                self.scroll = Some(Scroll::End);
            } else if !text.is_empty() {
                self.link = text;
                self.focus_link = true;
            }
        }
        let nothing_focused = ctx.memory(|m| m.focused().is_none());
        if nothing_focused
            && (room.current_track().is_some() || !room.queue.is_empty())
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Space))
        {
            self.send(Command::TogglePlay);
        }
    }

    fn add_bar_ui(&mut self, ui: &mut egui::Ui) {
        let id = egui::Id::unique("link_input");
        let btn_w = 108.0;
        let mut submit = false;
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.link)
                    .id(id)
                    .hint_text(s::PASTE_LINK)
                    .font(font(14.5))
                    .margin(vec2(12.0, 9.0))
                    .desired_width((ui.available_width() - btn_w - 10.0).max(120.0)),
            );
            if std::mem::take(&mut self.focus_link) {
                r.request_focus();
            }
            let can_add = !self.link.trim().is_empty();
            let (icon, label) = if is_search(&self.link) {
                (Icon::Search, s::SEARCH)
            } else {
                (Icon::Plus, s::ADD)
            };
            let enter = enter_pressed(ui, &r);
            let h = r.rect.height().max(36.0);
            let clicked = primary_button(ui, Some(icon), label, vec2(btn_w, h), can_add).clicked();
            if can_add && (clicked || enter) {
                submit = true;
                // Keep the field focused so several links can be added in a row.
                ui.memory_mut(|m| m.request_focus(id));
            }
        });
        ui.add_space(-2.0);
        ui.label(
            RichText::new(s::PASTE_HINT)
                .size(11.5)
                .color(theme::TEXT_FAINT),
        );
        if submit {
            let text = self.link.trim().to_string();
            if is_search(&text) {
                // The query stays in the field, so it can be refined.
                self.send(Command::Search {
                    query: text,
                    playlists: self.search_playlists,
                });
                self.search_open = true;
                self.search_added.clear();
            } else {
                self.link.clear();
                self.send(Command::Add(text));
                self.search_open = false;
                self.scroll = Some(Scroll::End);
            }
        }
    }

    /// YouTube search results, each with a button that adds it to the queue.
    fn search_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let search = &view.search;
        let mut close = false;
        let mut mode = None;
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(s::SEARCH_TITLE)
                    .font(bold_font(15.5))
                    .color(theme::TEXT),
            );
            ui.add_space(4.0);
            for (playlists, text) in [(false, s::SEARCH_SONGS), (true, s::SEARCH_PLAYLISTS)] {
                if toggle_pill(ui, text, self.search_playlists == playlists).clicked()
                    && self.search_playlists != playlists
                {
                    mode = Some(playlists);
                }
            }
            if !search.query.is_empty() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("· {}", search.query))
                            .size(13.0)
                            .color(theme::TEXT_DIM),
                    )
                    .truncate(),
                );
            }
            if !search.results.is_empty() && ui.available_width() > 360.0 {
                let hint = if search.playlists {
                    s::SEARCH_HINT_PLAYLISTS
                } else {
                    s::SEARCH_HINT
                };
                ui.label(
                    RichText::new(format!("· {hint}"))
                        .size(12.0)
                        .color(theme::TEXT_FAINT),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(egui::Button::new(s::BACK_TO_QUEUE).min_size(vec2(0.0, 28.0)))
                    .clicked()
                {
                    close = true;
                }
            });
        });
        ui.add_space(4.0);
        let escape = !self.settings_open
            && !ui.ctx().egui_wants_keyboard_input()
            && ui.input(|i| i.key_pressed(Key::Escape));
        if close || escape {
            self.search_open = false;
            self.send(Command::ClearSearch);
            return;
        }
        if let Some(playlists) = mode {
            self.search_playlists = playlists;
            self.config.search_playlists = playlists;
            self.config.save();
            if !search.query.is_empty() {
                self.send(Command::Search {
                    query: search.query.clone(),
                    playlists,
                });
                return;
            }
        }
        if let Some(e) = &search.error {
            empty_state(ui, e, "");
            return;
        }
        if search.loading || search.results.is_empty() {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(26.0).color(theme::accent()));
                ui.add_space(10.0);
                ui.label(
                    RichText::new(s::SEARCHING)
                        .size(13.0)
                        .color(theme::TEXT_DIM),
                );
            });
            return;
        }

        let in_queue: HashSet<&str> = room.queue.iter().map(|t| t.youtube_id.as_str()).collect();
        let hits = &search.results;
        ui.spacing_mut().item_spacing.y = ROW_GAP;
        egui::ScrollArea::vertical()
            // A new search starts scrolled to the top.
            .id_salt(("search_scroll", &search.query, search.playlists))
            .auto_shrink([false, false])
            .show_rows(ui, ROW_H, hits.len(), |ui, range| {
                for hit in &hits[range] {
                    let added = in_queue.contains(hit.youtube_id.as_str())
                        || self.search_added.contains(&hit.youtube_id);
                    self.search_row(ui, hit, added);
                }
            });
    }

    fn search_row(&mut self, ui: &mut egui::Ui, hit: &SearchHit, added: bool) {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let p = ui.painter().clone();
        if ui.rect_contains_pointer(rect) {
            p.rect_filled(rect, 8.0, theme::CARD_HOVER);
        }
        let thumb = Rect::from_min_size(
            pos2(rect.left() + 8.0, rect.center().y - 18.0),
            vec2(64.0, 36.0),
        );
        let tex = hit.thumb_id.as_deref().and_then(|id| self.thumbs.get(id));
        if hit.playlist {
            // Two cards peeking out behind the cover: a stack of songs.
            for (k, a) in [(2.0, 60), (1.0, 120)] {
                let r = thumb
                    .translate(vec2(0.0, -3.0 * k))
                    .shrink2(vec2(4.0 * k, 0.0));
                p.rect_filled(r, 4.0, theme::accent_alpha(a));
            }
        }
        paint_thumb(ui, thumb, tex, 5);

        let btn =
            Rect::from_center_size(pos2(rect.right() - 24.0, rect.center().y), vec2(30.0, 30.0));
        let btn_resp = ui.interact(
            btn,
            ui.scope_id().with(("search_add", &hit.youtube_id)),
            Sense::click(),
        );
        let add = if added {
            p.circle_filled(btn.center(), 15.0, theme::accent_alpha(36));
            icons::paint(&p, Icon::Check, btn.shrink(9.0), theme::accent());
            btn_resp.on_hover_text(s::IN_QUEUE);
            false
        } else {
            let fill = if btn_resp.hovered() {
                theme::accent_hover()
            } else {
                theme::accent()
            };
            p.circle_filled(btn.center(), 15.0, fill);
            icons::paint(&p, Icon::Plus, btn.shrink(9.5), theme::ON_ACCENT);
            let btn_resp = btn_resp
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(if hit.playlist {
                    s::ADD_PLAYLIST
                } else {
                    s::ADD_TO_QUEUE
                });
            btn_resp.clicked() || resp.double_clicked()
        };

        let dur_right = btn.left() - 14.0;
        let text_left = thumb.right() + 12.0;
        let max_w = (dur_right - 56.0 - text_left).max(40.0);
        let title = one_line(ui, &hit.title, font(14.0), theme::TEXT, max_w);
        p.galley(
            pos2(text_left, rect.center().y - title.size().y - 1.0),
            title,
            theme::TEXT,
        );
        if let Some(ch) = &hit.channel {
            let ch = one_line(ui, ch, font(12.0), theme::TEXT_DIM, max_w);
            p.galley(pos2(text_left, rect.center().y + 2.0), ch, theme::TEXT_DIM);
        }
        if hit.playlist {
            let g = p.layout_no_wrap(
                s::PLAYLIST_TAG.to_uppercase(),
                bold_font(10.5),
                theme::accent_hover(),
            );
            let r = Rect::from_min_size(
                pos2(dur_right - g.size().x - 14.0, rect.center().y - 10.0),
                vec2(g.size().x + 14.0, 20.0),
            );
            p.rect(
                r,
                10.0,
                theme::accent_alpha(28),
                Stroke::new(1.0, theme::accent_alpha(90)),
                StrokeKind::Inside,
            );
            p.galley(r.center() - g.size() / 2.0, g, theme::accent_hover());
        } else if let Some(d) = hit.duration_ms {
            p.text(
                pos2(dur_right, rect.center().y),
                Align2::RIGHT_CENTER,
                fmt_time(d),
                font(12.5),
                theme::TEXT_FAINT,
            );
        }
        if add {
            self.send(Command::Add(hit.url()));
            self.search_added.insert(hit.youtube_id.clone());
        }
    }

    fn now_playing_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let track = room.current_track().cloned();
        // Filled in once the card's size is known: above its fill, below
        // its content.
        let mut wash = None;
        let card = card_frame().show(ui, |ui| {
            wash = Some(ui.painter().add(egui::Shape::Noop));
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(144.0, 81.0), Sense::hover());
                match &track {
                    Some(t) => {
                        let tex = self.thumbs.get(&t.youtube_id);
                        paint_thumb(ui, r, tex, 8);
                    }
                    None => {
                        ui.painter().rect_filled(r, 8.0, theme::accent_alpha(28));
                        let c = r.center();
                        icons::paint(
                            ui.painter(),
                            Icon::Note,
                            Rect::from_center_size(c, vec2(30.0, 30.0)),
                            theme::accent(),
                        );
                    }
                }
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(s::NOW_PLAYING.to_uppercase())
                            .font(bold_font(11.0))
                            .color(theme::accent()),
                    );
                    match &track {
                        Some(t) => {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&t.title)
                                        .font(bold_font(18.0))
                                        .color(theme::TEXT),
                                )
                                .truncate(),
                            )
                            .on_hover_text(&t.title);
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                ui.label(
                                    RichText::new(s::ADDED_BY).size(12.5).color(theme::TEXT_DIM),
                                );
                                ui.label(
                                    RichText::new(&t.added_by)
                                        .size(12.5)
                                        .color(theme::name_color(&t.added_by)),
                                );
                            });
                        }
                        None => {
                            ui.label(
                                RichText::new(s::NOTHING_PLAYING_TITLE)
                                    .font(bold_font(18.0))
                                    .color(theme::TEXT),
                            );
                            ui.label(
                                RichText::new(s::NOTHING_PLAYING_HINT)
                                    .size(12.5)
                                    .color(theme::TEXT_DIM),
                            );
                        }
                    }
                    if track.is_some() {
                        ui.add_space(2.0);
                        let (text, color) = status(view);
                        tag(
                            ui,
                            &text,
                            color.gamma_multiply(0.12),
                            color.gamma_multiply(0.35),
                            theme::TEXT,
                            Some(color),
                        );
                    }
                });
            });
            ui.add_space(10.0);
            self.progress_ui(ui, view, track.as_ref());
            ui.add_space(4.0);
            self.transport_ui(ui, room, track.is_some());
            if let Some(e) = &view.player_error {
                ui.add_space(6.0);
                error_box(ui, &format!("{e} {}", s::MISSING_TOOLS_PLAYBACK));
            }
        });
        if let (Some(idx), true) = (wash, track.is_some()) {
            ui.painter()
                .set(idx, accent_wash(card.response.rect.shrink(1.0), 11));
        }
    }

    fn progress_ui(&mut self, ui: &mut egui::Ui, view: &View, track: Option<&shared::Track>) {
        let (shown, duration) = self.seek_bar(ui, view, track, true);
        ui.add_space(-6.0);
        ui.horizontal(|ui| {
            let small = |t: String| RichText::new(t).size(12.0).color(theme::TEXT_DIM);
            ui.label(small(if track.is_some() {
                fmt_time(shown)
            } else {
                "0:00".into()
            }));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small(duration.map_or("--:--".into(), fmt_time)));
            });
        });
    }

    /// The seek bar; returns the position shown and the song length.
    fn seek_bar(
        &mut self,
        ui: &mut egui::Ui,
        view: &View,
        track: Option<&shared::Track>,
        bubble: bool,
    ) -> (u64, Option<u64>) {
        let duration = track.and_then(|t| t.duration_ms).filter(|d| *d > 0);
        let mut pos = view.position_ms().unwrap_or(0);
        if let Some((target, at)) = self.seek_hold {
            if at.elapsed() < SEEK_HOLD && pos.abs_diff(target) > 1200 {
                pos = target;
            } else {
                self.seek_hold = None;
            }
        }
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::click_and_drag());
        self.bar_width = rect.width();
        let pointer_frac = resp
            .interact_pointer_pos()
            .or(resp.hover_pos())
            .map(|p| ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        if let Some(d) = duration {
            if resp.dragged() {
                self.seek_drag = pointer_frac;
            }
            if resp.drag_stopped() || resp.clicked() {
                if let Some(f) = pointer_frac.or(self.seek_drag) {
                    let target = (f as f64 * d as f64) as u64;
                    self.send(Command::Seek(target));
                    self.seek_hold = Some((target, Instant::now()));
                    pos = target;
                }
                self.seek_drag = None;
            }
        } else {
            self.seek_drag = None;
        }
        let shown = match (self.seek_drag, duration) {
            (Some(f), Some(d)) => (f as f64 * d as f64) as u64,
            _ => pos,
        };
        let frac = duration.map_or(0.0, |d| (shown as f32 / d as f32).clamp(0.0, 1.0));
        let active = duration.is_some() && (resp.hovered() || resp.dragged());
        let h = if active { 6.0 } else { 4.0 };
        let bar = Rect::from_center_size(rect.center(), vec2(rect.width(), h));
        let p = ui.painter();
        p.rect_filled(bar, h / 2.0, theme::BORDER);
        let done = Rect::from_min_max(bar.min, pos2(bar.left() + bar.width() * frac, bar.bottom()));
        if done.width() > 0.5 {
            p.rect_filled(done, h / 2.0, theme::accent());
        }
        if active {
            p.circle_filled(pos2(done.right(), bar.center().y), 7.0, Color32::WHITE);
            if let (true, Some(f), Some(d)) = (bubble, pointer_frac, duration) {
                let text = fmt_time((f as f64 * d as f64) as u64);
                let g = p.layout_no_wrap(text, font(12.0), theme::TEXT);
                let x = (rect.left() + rect.width() * f).clamp(
                    rect.left() + g.size().x / 2.0 + 8.0,
                    rect.right() - g.size().x / 2.0 - 8.0,
                );
                let bubble =
                    Rect::from_center_size(pos2(x, rect.top() - 12.0), g.size() + vec2(14.0, 6.0));
                p.rect(
                    bubble,
                    6.0,
                    theme::BG,
                    Stroke::new(1.0, theme::BORDER),
                    StrokeKind::Inside,
                );
                p.galley(bubble.center() - g.size() / 2.0, g, theme::TEXT);
            }
            resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
        (shown, duration)
    }

    fn transport_ui(&mut self, ui: &mut egui::Ui, room: &RoomState, has_track: bool) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            self.playback_buttons(ui, room, has_track, false);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.spacing_mut().slider_width = 110.0;
                self.volume_ui(ui);
                ui.add_space(6.0);
                let on = self.config.normalize;
                if toggle_pill(ui, s::NORMALIZE, on)
                    .on_hover_text(if on {
                        s::NORMALIZE_ON
                    } else {
                        s::NORMALIZE_OFF
                    })
                    .clicked()
                {
                    self.config.normalize = !on;
                    self.config.save();
                    self.send(Command::SetNormalize(!on));
                }
            });
        });
    }

    /// Previous, play/pause, next.
    fn playback_buttons(
        &mut self,
        ui: &mut egui::Ui,
        room: &RoomState,
        has_track: bool,
        compact: bool,
    ) {
        let (side, main) = if compact { (28.0, 32.0) } else { (36.0, 46.0) };
        if icons::ghost(ui, Icon::Previous, side, has_track)
            .on_hover_text(s::PREVIOUS)
            .clicked()
        {
            self.send(Command::Previous);
        }
        let playing = room.playback.status == PlayStatus::Playing || room.waiting_for_ready;
        let (icon, tip) = if playing {
            (Icon::Pause, s::PAUSE)
        } else {
            (Icon::Play, s::PLAY)
        };
        if icons::round(ui, icon, main, has_track || !room.queue.is_empty())
            .on_hover_text(tip)
            .clicked()
        {
            self.send(Command::TogglePlay);
        }
        if icons::ghost(ui, Icon::Next, side, has_track)
            .on_hover_text(s::NEXT)
            .clicked()
        {
            self.send(Command::Next);
        }
    }

    /// Mute button, volume slider and percentage (in the layout's direction).
    fn volume_ui(&mut self, ui: &mut egui::Ui) {
        if ui.layout().prefer_right_to_left() {
            self.volume_label(ui);
            self.volume_slider(ui);
            self.mute_button(ui);
        } else {
            self.mute_button(ui);
            self.volume_slider(ui);
            self.volume_label(ui);
        }
    }

    fn volume_label(&self, ui: &mut egui::Ui) {
        ui.add_sized(
            [36.0, 20.0],
            egui::Label::new(
                RichText::new(format!("{:.0}%", self.volume))
                    .size(12.5)
                    .color(theme::TEXT_DIM),
            ),
        );
    }

    fn volume_slider(&mut self, ui: &mut egui::Ui) {
        let r = ui.add(
            egui::Slider::new(&mut self.volume, 0.0..=100.0)
                .show_value(false)
                .step_by(1.0),
        );
        if r.changed() {
            self.muted_from = None;
            self.send(Command::SetVolume(self.volume));
        }
        if r.drag_stopped() || (r.changed() && !r.dragged()) {
            self.config.volume = self.volume;
            self.config.save();
        }
    }

    fn mute_button(&mut self, ui: &mut egui::Ui) {
        let muted = self.volume <= 0.0;
        let (icon, tip) = if muted {
            (Icon::Mute, s::UNMUTE)
        } else {
            (Icon::Volume, s::MUTE)
        };
        if icons::ghost(ui, icon, 30.0, true)
            .on_hover_text(tip)
            .clicked()
        {
            if muted {
                let v = self.muted_from.take().unwrap_or(50.0);
                self.set_volume(v);
            } else {
                self.muted_from = Some(self.volume);
                self.set_volume(0.0);
            }
        }
    }

    fn queue_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let current = room.playback.track_index;
        let len = room.queue.len();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(s::QUEUE)
                    .font(bold_font(15.5))
                    .color(theme::TEXT),
            );
            if len > 0 {
                let total: u64 = room.queue.iter().filter_map(|t| t.duration_ms).sum();
                let mut text = s::songs(len);
                if total > 0 {
                    text.push_str(" · ");
                    text.push_str(&fmt_time(total));
                }
                text.push_str(" · ");
                text.push_str(s::DOUBLE_CLICK_HINT);
                ui.label(RichText::new(text).size(12.0).color(theme::TEXT_FAINT));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if current.is_some()
                    && icons::ghost(ui, Icon::Note, 30.0, true)
                        .on_hover_text(s::JUMP_TO_CURRENT)
                        .clicked()
                {
                    self.scroll = current.map(Scroll::Index);
                }
                if len > 0 {
                    if !self.filter.is_empty()
                        && icons::ghost(ui, Icon::Remove, 26.0, true).clicked()
                    {
                        self.filter.clear();
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .hint_text(s::SEARCH_QUEUE)
                            .font(font(13.0))
                            .margin(vec2(8.0, 5.0))
                            .desired_width(160.0),
                    );
                    icons::label(ui, Icon::Search, 18.0, theme::TEXT_FAINT);
                }
            });
        });
        ui.add_space(4.0);

        let q = self.filter.trim().to_lowercase();
        let rows: Vec<usize> = (0..len)
            .filter(|&i| {
                q.is_empty()
                    || room.queue[i].title.to_lowercase().contains(&q)
                    || room.queue[i].added_by.to_lowercase().contains(&q)
            })
            .collect();
        let pending: &[PendingAdd] = if q.is_empty() {
            &view.pending_adds
        } else {
            &[]
        };
        let n = rows.len() + pending.len();
        if n == 0 {
            let (title, hint) = if q.is_empty() {
                (s::QUEUE_EMPTY_TITLE, s::QUEUE_EMPTY_HINT)
            } else {
                (s::NO_MATCHES, "")
            };
            empty_state(ui, title, hint);
            self.drag = None;
            return;
        }
        // Reordering works on the full list only (not on search results).
        let can_drag = q.is_empty();
        if !can_drag || ui.input(|i| i.key_pressed(Key::Escape)) {
            self.drag = None;
        }

        ui.spacing_mut().item_spacing.y = ROW_GAP;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("queue_scroll")
            .auto_shrink([false, false]);
        let step = ROW_H + ROW_GAP;
        match self.scroll.take() {
            Some(Scroll::Index(i)) => {
                if let Some(pos) = rows.iter().position(|&r| r == i) {
                    let view_h = ui.available_height();
                    let off = pos as f32 * step - (view_h - ROW_H) / 2.0;
                    area = area.vertical_scroll_offset(off.max(0.0));
                }
            }
            Some(Scroll::End) => {
                let view_h = ui.available_height();
                let off = (n as f32 + 2.0) * step - view_h;
                area = area.vertical_scroll_offset(off.max(0.0));
            }
            None => {}
        }
        let playing = room.playback.status == PlayStatus::Playing;
        area.show_rows(ui, ROW_H, n, |ui, range| {
            let content_top = ui.cursor().top() - range.start as f32 * step;
            for r in range {
                if let Some(&i) = rows.get(r) {
                    self.queue_row(ui, room, i, current, playing, can_drag);
                } else if let Some(p) = pending.get(r - rows.len()) {
                    pending_row(ui, p);
                }
            }
            self.drag_feedback(ui, room, content_top, step);
        });
        self.finish_drag(ui, room);
    }

    /// While dragging: tracks the drop slot, scrolls near the edges, and
    /// draws the drop line and the floating row.
    fn drag_feedback(&mut self, ui: &mut egui::Ui, room: &RoomState, content_top: f32, step: f32) {
        let Some(d) = &mut self.drag else { return };
        let Some(ptr) = ui.ctx().pointer_latest_pos() else {
            return;
        };
        let len = room.queue.len();
        let Some(from) = room.queue.iter().position(|t| t.id == d.id) else {
            self.drag = None;
            return;
        };
        d.slot = ((ptr.y - content_top) / step + 0.5)
            .floor()
            .clamp(0.0, len as f32) as usize;
        let clip = ui.clip_rect();
        let edge = 40.0;
        let dy = if ptr.y < clip.top() + edge {
            (clip.top() + edge - ptr.y).min(edge) * 0.35
        } else if ptr.y > clip.bottom() - edge {
            -(ptr.y - (clip.bottom() - edge)).min(edge) * 0.35
        } else {
            0.0
        };
        if dy != 0.0 {
            ui.scroll_with_delta(vec2(0.0, dy));
        }
        ui.ctx().request_repaint();

        let left = ui.max_rect().left();
        let right = ui.max_rect().right();
        if d.slot != from && d.slot != from + 1 {
            let y = (content_top + d.slot as f32 * step - ROW_GAP / 2.0)
                .clamp(clip.top() + 1.0, clip.bottom() - 1.0);
            let p = ui.painter();
            p.line_segment(
                [pos2(left + 8.0, y), pos2(right - 4.0, y)],
                Stroke::new(2.0, theme::accent()),
            );
            p.circle_filled(pos2(left + 8.0, y), 4.0, theme::accent());
        }

        // The row follows the pointer above everything else.
        let t = &room.queue[from];
        let top = ptr.y - d.grab_dy;
        let rect = Rect::from_min_size(pos2(left, top), vec2(right - left, ROW_H));
        let p = ui
            .ctx()
            .layer_painter(LayerId::new(Order::Tooltip, Id::unique("queue_drag")));
        p.rect(
            rect,
            8.0,
            theme::CARD_HOVER,
            Stroke::new(1.0, theme::accent()),
            StrokeKind::Inside,
        );
        let thumb =
            Rect::from_min_size(pos2(left + 40.0, rect.center().y - 18.0), vec2(64.0, 36.0));
        match self.thumbs.get(&t.youtube_id) {
            Some(tex) => {
                p.image(
                    tex,
                    thumb,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                p.rect_filled(thumb, 5.0, theme::CARD);
            }
        }
        let text_left = thumb.right() + 12.0;
        let title = one_line(
            ui,
            &t.title,
            font(14.0),
            theme::TEXT,
            (right - text_left - 16.0).max(40.0),
        );
        p.galley(
            pos2(text_left, rect.center().y - title.size().y / 2.0),
            title,
            theme::TEXT,
        );
    }

    /// On release: moves the dragged track to the slot under the pointer.
    fn finish_drag(&mut self, ui: &egui::Ui, room: &RoomState) {
        if self.drag.is_none() {
            return;
        }
        if ui.input(|i| i.pointer.primary_down()) {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            return;
        }
        let Some(d) = self.drag.take() else { return };
        let Some(from) = room.queue.iter().position(|t| t.id == d.id) else {
            return;
        };
        let to = if d.slot > from { d.slot - 1 } else { d.slot };
        let to = to.min(room.queue.len().saturating_sub(1));
        if to != from {
            self.send(Command::Move(d.id, to));
        }
    }

    fn queue_row(
        &mut self,
        ui: &mut egui::Ui,
        room: &RoomState,
        i: usize,
        current: Option<usize>,
        playing: bool,
        can_drag: bool,
    ) {
        let t = &room.queue[i];
        let len = room.queue.len();
        let is_cur = current == Some(i);
        let sense = if can_drag {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
        let dragging = self.drag.is_some();
        let is_dragged = self.drag.as_ref().is_some_and(|d| d.id == t.id);
        let hovered = ui.rect_contains_pointer(rect) && !dragging;
        let p = ui.painter().clone();
        if is_cur {
            p.rect_filled(rect, 8.0, theme::accent_alpha(30));
            p.rect_filled(
                Rect::from_min_size(rect.min + vec2(0.0, 12.0), vec2(3.0, ROW_H - 24.0)),
                2.0,
                theme::accent(),
            );
        } else if hovered {
            p.rect_filled(rect, 8.0, theme::CARD);
        }
        let num = Rect::from_min_size(rect.min, vec2(40.0, ROW_H));
        if is_cur {
            paint_bars(&p, num.center(), playing);
        } else {
            p.text(
                num.center(),
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                font(12.5),
                theme::TEXT_FAINT,
            );
        }
        let thumb =
            Rect::from_min_size(pos2(num.right(), rect.center().y - 18.0), vec2(64.0, 36.0));
        let tex = self.thumbs.get(&t.youtube_id);
        paint_thumb(ui, thumb, tex, 5);
        // The thumbnail is a play button (play/pause for the current song).
        let thumb_resp = ui.interact(thumb, ui.scope_id().with(("thumb", t.id)), sense);
        if thumb_resp.hovered() && !dragging {
            p.rect_filled(thumb, 5.0, Color32::from_black_alpha(140));
            let icon = if is_cur && playing {
                Icon::Pause
            } else {
                Icon::Play
            };
            icons::paint(
                &p,
                icon,
                Rect::from_center_size(thumb.center(), vec2(16.0, 16.0)),
                Color32::WHITE,
            );
            ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        }
        let thumb_resp = if is_cur {
            thumb_resp
        } else {
            thumb_resp.on_hover_text(s::PLAY_THIS)
        };
        if thumb_resp.clicked() {
            if !is_cur {
                self.send(Command::PlayTrack(t.id));
            } else if playing {
                self.send(Command::Pause);
            } else {
                self.send(Command::Play);
            }
        }
        if can_drag && (resp.drag_started() || thumb_resp.drag_started()) {
            if let Some(ptr) = ui.ctx().pointer_interact_pos() {
                self.drag = Some(QueueDrag {
                    id: t.id,
                    grab_dy: ptr.y - rect.top(),
                    slot: i,
                });
            }
        }
        if can_drag && resp.hovered() && !dragging {
            ui.ctx().set_cursor_icon(CursorIcon::Grab);
        }

        let actions_w = if hovered { 4.0 * 28.0 + 6.0 } else { 0.0 };
        let dur_right = rect.right() - actions_w - 12.0;
        let text_left = thumb.right() + 12.0;
        let max_w = (dur_right - 56.0 - text_left).max(40.0);
        let title_font = if is_cur { bold_font(14.0) } else { font(14.0) };
        let title = one_line(ui, &t.title, title_font, theme::TEXT, max_w);
        p.galley(
            pos2(text_left, rect.center().y - title.size().y - 1.0),
            title,
            theme::TEXT,
        );
        let by = one_line(
            ui,
            &t.added_by,
            font(12.0),
            theme::name_color(&t.added_by).gamma_multiply(0.85),
            max_w,
        );
        p.galley(pos2(text_left, rect.center().y + 2.0), by, theme::TEXT_DIM);
        p.text(
            pos2(dur_right, rect.center().y),
            Align2::RIGHT_CENTER,
            t.duration_ms.map_or("…".into(), fmt_time),
            font(12.5),
            theme::TEXT_FAINT,
        );

        if hovered {
            let arect = Rect::from_min_max(
                pos2(rect.right() - actions_w - 4.0, rect.top()),
                pos2(rect.right() - 6.0, rect.bottom()),
            );
            let mut ui = ui.new_child(
                UiBuilder::new()
                    .id_salt(("row_actions", t.id))
                    .max_rect(arect)
                    .layout(Layout::right_to_left(Align::Center)),
            );
            ui.spacing_mut().item_spacing.x = 2.0;
            if icons::ghost(&mut ui, Icon::Remove, 26.0, true)
                .on_hover_text(s::REMOVE)
                .clicked()
            {
                self.send(Command::Remove(t.id));
            }
            if icons::ghost(&mut ui, Icon::Down, 26.0, i + 1 < len)
                .on_hover_text(s::MOVE_DOWN)
                .clicked()
            {
                self.send(Command::Move(t.id, i + 1));
            }
            if icons::ghost(&mut ui, Icon::Up, 26.0, i > 0)
                .on_hover_text(s::MOVE_UP)
                .clicked()
            {
                self.send(Command::Move(t.id, i - 1));
            }
            // Final index that puts this track right after the current one.
            let next_target = match current {
                Some(c) if i < c => Some(c),
                Some(c) if i > c + 1 => Some(c + 1),
                None if i > 0 => Some(0),
                _ => None,
            };
            if icons::ghost(&mut ui, Icon::PlayNext, 26.0, next_target.is_some())
                .on_hover_text(s::PLAY_NEXT)
                .clicked()
            {
                if let Some(to) = next_target {
                    self.send(Command::Move(t.id, to));
                }
            }
        }
        if resp.double_clicked() && !is_cur {
            self.send(Command::PlayTrack(t.id));
        }
        if is_dragged {
            // The row floats under the pointer; leave a faded gap here.
            p.rect_filled(rect, 8.0, theme::BG.gamma_multiply(0.75));
        }
    }

    fn toasts_ui(&self, ctx: &egui::Context, view: &View) {
        let live: Vec<_> = view
            .toasts
            .iter()
            .filter(|t| t.at.elapsed().as_secs_f32() < TOAST_SECS)
            .collect();
        if live.is_empty() {
            return;
        }
        // In the room: bottom-right of the main area, clear of the sidebar.
        let offset = if matches!(self.screen, Screen::Room) {
            vec2(-(SIDEBAR_W + 16.0), -96.0)
        } else {
            vec2(-16.0, -40.0)
        };
        let mut animating = false;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, offset)
            .interactable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for t in live.iter().rev().take(4).rev() {
                    let age = t.at.elapsed().as_secs_f32();
                    let fade_in = (age / TOAST_FADE).min(1.0);
                    let fade_out = ((TOAST_SECS - age) / TOAST_FADE).min(1.0);
                    let alpha = fade_in.min(fade_out).clamp(0.0, 1.0);
                    animating |= alpha < 1.0;
                    ui.scope(|ui| {
                        ui.multiply_opacity(alpha);
                        let color = if t.error { theme::BAD } else { theme::accent() };
                        Frame::new()
                            .fill(theme::CARD)
                            .stroke(Stroke::new(1.0, theme::BORDER))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin {
                                left: 14,
                                right: 14,
                                top: 10,
                                bottom: 10,
                            })
                            .shadow(ui.visuals().popup_shadow)
                            .show(ui, |ui| {
                                ui.set_max_width(320.0);
                                ui.horizontal(|ui| {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(3.0, 18.0), Sense::hover());
                                    ui.painter().rect_filled(r, 2.0, color);
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&t.text).color(if t.error {
                                                theme::BAD
                                            } else {
                                                theme::TEXT
                                            }),
                                        )
                                        .wrap(),
                                    );
                                });
                            });
                    });
                }
            });
        if animating {
            ctx.request_repaint();
        } else {
            let soonest = live
                .iter()
                .map(|t| TOAST_SECS - TOAST_FADE - t.at.elapsed().as_secs_f32())
                .fold(TOAST_SECS, f32::min);
            ctx.request_repaint_after(Duration::from_secs_f32(soonest.max(0.02)));
        }
    }
}

// ----- recent rooms ------------------------------------------------------------------

enum RecentAction {
    Join(usize),
    Forget(usize),
}

const RECENT_ROW_H: f32 = 48.0;
/// Rows shown before the list scrolls.
const RECENT_VISIBLE: f32 = 4.5;

/// The "recent rooms" list under the connect button.
fn recent_rooms_ui(
    ui: &mut egui::Ui,
    recent: &[crate::config::RecentRoom],
) -> Option<RecentAction> {
    let mut action = None;
    ui.add_space(18.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0.0, theme::BORDER);
    ui.add_space(12.0);
    section_label(ui, s::RECENT_ROOMS);
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("recent_rooms")
        // Shrink to the space left in short windows (at least one row).
        .max_height(
            (RECENT_ROW_H * RECENT_VISIBLE).min(ui.available_height().max(RECENT_ROW_H + 4.0)),
        )
        .auto_shrink([false, true])
        // Show the bar whenever rows are hidden (small windows), not only on hover.
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, r) in recent.iter().enumerate() {
                if let Some(a) = recent_row(ui, i, r) {
                    action = Some(a);
                }
            }
        });
    action
}

fn recent_row(ui: &mut egui::Ui, i: usize, r: &crate::config::RecentRoom) -> Option<RecentAction> {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), RECENT_ROW_H), Sense::click());
    let hovered = ui.rect_contains_pointer(rect);
    let p = ui.painter().clone();
    if hovered {
        p.rect_filled(rect, 8.0, theme::CARD_HOVER);
    }
    let c = theme::name_color(&r.room);
    let avatar = pos2(rect.left() + 22.0, rect.center().y);
    p.circle_filled(avatar, 15.0, c.gamma_multiply(0.22));
    let initial: String = r
        .room
        .chars()
        .next()
        .map(|ch| ch.to_uppercase().collect())
        .unwrap_or_default();
    p.text(avatar, Align2::CENTER_CENTER, initial, bold_font(13.0), c);

    let text_left = rect.left() + 46.0;
    let max_w = (rect.right() - 40.0 - text_left).max(40.0);
    let title = one_line(ui, &r.room, bold_font(14.0), theme::TEXT, max_w);
    p.galley(
        pos2(text_left, rect.center().y - title.size().y - 1.0),
        title,
        theme::TEXT,
    );
    let sub = format!("{} · {}", r.name, r.server_host());
    let sub = one_line(ui, &sub, font(12.0), theme::TEXT_DIM, max_w);
    p.galley(pos2(text_left, rect.center().y + 2.0), sub, theme::TEXT_DIM);

    let mut forget = false;
    if hovered {
        let arect = Rect::from_min_max(
            pos2(rect.right() - 36.0, rect.top()),
            pos2(rect.right() - 6.0, rect.bottom()),
        );
        let mut ui = ui.new_child(
            UiBuilder::new()
                .id_salt(("recent_forget", i))
                .max_rect(arect)
                .layout(Layout::right_to_left(Align::Center)),
        );
        forget = icons::ghost(&mut ui, Icon::Remove, 26.0, true)
            .on_hover_text(s::RECENT_FORGET)
            .clicked();
    }
    if forget {
        Some(RecentAction::Forget(i))
    } else if resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(s::RECENT_JOIN)
        .clicked()
    {
        Some(RecentAction::Join(i))
    } else {
        None
    }
}

// ----- settings ----------------------------------------------------------------------

const EQ_FREQ_LABELS: [&str; crate::eq::BANDS] = [
    "32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

impl App {
    /// The settings window (equalizer and video mode), over the room screen.
    fn settings_ui(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let w = (screen.width() - 32.0).clamp(260.0, 620.0);
        let margin = if w < 400.0 { 14 } else { 22 };
        let mut close = false;
        let resp = egui::Modal::new(Id::unique("settings_modal"))
            .backdrop_color(Color32::from_black_alpha(150))
            .frame(
                card_frame()
                    .inner_margin(Margin::same(margin))
                    .shadow(ctx.global_style().visuals.window_shadow),
            )
            .show(ctx, |ui| {
                let inner_w = w - 2.0 * margin as f32 - 2.0;
                ui.set_width(inner_w);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                    icons::paint(ui.painter(), Icon::Gear, r.shrink(2.0), theme::accent());
                    ui.label(
                        RichText::new(s::SETTINGS)
                            .font(bold_font(18.0))
                            .color(theme::TEXT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::ghost(ui, Icon::Remove, 28.0, true)
                            .on_hover_text(s::CLOSE)
                            .clicked()
                        {
                            close = true;
                        }
                    });
                });
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .max_height((screen.height() - 120.0).max(200.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(inner_w);
                        section_label(ui, s::EQ_TITLE);
                        ui.add_space(6.0);
                        self.eq_ui(ui);
                        ui.add_space(18.0);
                        ui.separator();
                        ui.add_space(12.0);
                        section_label(ui, s::VIDEO_TITLE);
                        ui.add_space(6.0);
                        let on = self.config.video;
                        if toggle_pill(ui, if on { s::ON } else { s::OFF }, on).clicked() {
                            self.config.video = !on;
                            self.config.save();
                            self.send(Command::SetVideo(!on));
                        }
                        ui.add_space(4.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(s::VIDEO_EXPLAIN)
                                    .size(12.5)
                                    .color(theme::TEXT_DIM),
                            )
                            .wrap(),
                        );
                    });
            });
        if close || resp.should_close() {
            self.settings_open = false;
            self.config.save();
        }
    }

    fn eq_ui(&mut self, ui: &mut egui::Ui) {
        use crate::eq::{BANDS, MAX_DB, PRESETS};
        let mut eq = self.config.eq.clone();
        let mut changed = false;
        let mut save = false;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if toggle_pill(ui, if eq.enabled { s::ON } else { s::OFF }, eq.enabled).clicked() {
                eq.enabled = !eq.enabled;
                changed = true;
                save = true;
            }
            ui.add_space(4.0);
            let current = eq.preset();
            for (i, (name, gains)) in PRESETS.iter().enumerate() {
                if chip(ui, name, eq.enabled && current == Some(i)).clicked() {
                    eq.gains = *gains;
                    eq.enabled = true;
                    changed = true;
                    save = true;
                }
            }
            if current.is_none() {
                chip(ui, s::EQ_CUSTOM, eq.enabled);
            }
        });
        ui.add_space(10.0);

        // The graph: one vertical slider per band, like a hardware EQ.
        let narrow = ui.available_width() < 420.0;
        let label_w = if narrow { 30.0 } else { 46.0 };
        let plot_h = if narrow { 170.0 } else { 200.0 };
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), plot_h + 40.0), Sense::hover());
        let plot = Rect::from_min_max(
            pos2(rect.left() + label_w, rect.top() + 14.0),
            pos2(rect.right() - 6.0, rect.top() + 14.0 + plot_h),
        );
        let half = plot.height() / 2.0;
        let y_of = |db: f64| plot.center().y - (db / MAX_DB) as f32 * half;
        let db_of = |y: f32| ((plot.center().y - y) / half) as f64 * MAX_DB;
        let col_w = plot.width() / BANDS as f32;
        let on = eq.enabled;
        let accent = if on {
            theme::accent()
        } else {
            theme::TEXT_FAINT
        };

        let p = ui.painter().clone();
        p.rect_filled(rect, 10.0, theme::FIELD);
        for (db, label) in [
            (12.0, "+12 dB"),
            (6.0, "+6"),
            (0.0, "0 dB"),
            (-6.0, "−6"),
            (-12.0, "−12 dB"),
        ] {
            let y = y_of(db);
            let stroke = if db == 0.0 {
                Stroke::new(1.0, theme::BORDER)
            } else {
                Stroke::new(1.0, theme::BORDER.gamma_multiply(0.45))
            };
            p.line_segment([pos2(plot.left() - 4.0, y), pos2(plot.right(), y)], stroke);
            let text = if narrow {
                label.trim_end_matches(" dB")
            } else {
                label
            };
            p.text(
                pos2(rect.left() + 8.0, y),
                Align2::LEFT_CENTER,
                text,
                font(10.5),
                theme::TEXT_FAINT,
            );
        }

        let mut knobs = Vec::with_capacity(BANDS);
        let mut active_band = None;
        for (i, freq_label) in EQ_FREQ_LABELS.iter().enumerate() {
            let x = plot.left() + col_w * (i as f32 + 0.5);
            let hit =
                Rect::from_center_size(pos2(x, plot.center().y), vec2(col_w, plot.height() + 28.0));
            let resp = ui
                .interact(
                    hit,
                    ui.scope_id().with(("eq_band", i)),
                    Sense::click_and_drag(),
                )
                .on_hover_cursor(CursorIcon::ResizeVertical);
            if resp.double_clicked() {
                eq.gains[i] = 0.0;
                changed = true;
            } else if resp.is_pointer_button_down_on() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    // 0.5 dB steps: smooth enough, and few player updates.
                    let v = (db_of(pos.y).clamp(-MAX_DB, MAX_DB) * 2.0).round() / 2.0;
                    if (v - eq.gains[i]).abs() > 0.01 || !eq.enabled {
                        eq.gains[i] = v;
                        eq.enabled = true;
                        changed = true;
                    }
                }
            }
            if resp.drag_stopped() || resp.clicked() || resp.double_clicked() {
                save = true;
            }
            let hot = resp.hovered() || resp.is_pointer_button_down_on();
            if hot {
                active_band = Some(i);
            }
            let ky = y_of(eq.gains[i]);
            p.line_segment(
                [pos2(x, plot.top()), pos2(x, plot.bottom())],
                Stroke::new(2.0, theme::BORDER),
            );
            p.line_segment(
                [pos2(x, y_of(0.0)), pos2(x, ky)],
                Stroke::new(3.0, accent.gamma_multiply(0.8)),
            );
            knobs.push((pos2(x, ky), hot));
            p.text(
                pos2(x, plot.bottom() + 16.0),
                Align2::CENTER_CENTER,
                *freq_label,
                font(if narrow { 10.0 } else { 11.0 }),
                if hot { theme::TEXT } else { theme::TEXT_DIM },
            );
        }
        // A soft curve through the knobs, then the knobs on top.
        let curve: Vec<_> = knobs.iter().map(|(k, _)| *k).collect();
        p.add(egui::Shape::line(
            curve,
            Stroke::new(
                1.5,
                if on {
                    theme::accent_alpha(110)
                } else {
                    theme::BORDER
                },
            ),
        ));
        let r = if narrow { 6.5 } else { 8.0 };
        for (k, hot) in &knobs {
            let rr = if *hot { r + 1.5 } else { r };
            p.circle_filled(*k, rr + 3.0, theme::FIELD);
            p.circle_filled(*k, rr, theme::CARD);
            p.circle_stroke(*k, rr, Stroke::new(2.5, accent));
            p.circle_filled(*k, rr * 0.38, accent);
        }
        if let Some(i) = active_band {
            let (k, _) = knobs[i];
            let text = format!("{:+.1} dB", eq.gains[i]);
            let g = p.layout_no_wrap(text, bold_font(11.0), theme::ON_ACCENT);
            let size = g.size() + vec2(12.0, 6.0);
            let mut bubble = Rect::from_center_size(pos2(k.x, k.y - r - 16.0), size);
            if bubble.top() < rect.top() + 2.0 {
                bubble = bubble.translate(vec2(0.0, 2.0 * (r + 16.0)));
            }
            let bubble = bubble.translate(vec2(
                (rect.left() + 2.0 - bubble.left()).max(0.0)
                    - (bubble.right() - rect.right() + 2.0).max(0.0),
                0.0,
            ));
            p.rect_filled(bubble, 6.0, theme::accent());
            p.galley(bubble.min + vec2(6.0, 3.0), g, theme::ON_ACCENT);
        }

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(s::EQ_HINT)
                    .size(12.0)
                    .color(theme::TEXT_FAINT),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        eq.gains.iter().any(|g| g.abs() > 0.01),
                        egui::Button::new(s::EQ_RESET).min_size(vec2(0.0, 26.0)),
                    )
                    .clicked()
                {
                    eq.gains = [0.0; BANDS];
                    changed = true;
                    save = true;
                }
            });
        });

        if changed {
            self.config.eq = eq.clone();
            self.send(Command::SetEq(eq));
        }
        if save {
            self.config.save();
        }
    }
}

/// A rounded choice chip (accent when selected).
fn chip(ui: &mut egui::Ui, text: &str, selected: bool) -> egui::Response {
    let color = if selected {
        theme::ON_ACCENT
    } else {
        theme::TEXT_DIM
    };
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font(12.5), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 22.0, 26.0), Sense::click());
    let (fill, stroke) = if selected {
        (theme::accent(), theme::accent())
    } else if resp.hovered() {
        (theme::CARD_HOVER, theme::BORDER)
    } else {
        (Color32::TRANSPARENT, theme::BORDER)
    };
    let p = ui.painter();
    p.rect(
        rect,
        13.0,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    p.galley(
        pos2(
            rect.center().x - g.size().x / 2.0,
            rect.center().y - g.size().y / 2.0,
        ),
        g,
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// ----- shared widgets ----------------------------------------------------------------

fn background_panel() -> egui::CentralPanel {
    egui::CentralPanel::default().frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
}

/// The song's colour as a soft wash over the left part of a card.
fn accent_wash(rect: Rect, radius: u8) -> egui::Shape {
    let strong = theme::accent_alpha(34);
    let split = rect.left() + radius as f32;
    let end = rect.left() + rect.width() * 0.72;
    // The rounded left edge, then a fade to nothing.
    let cap = egui::Shape::rect_filled(
        Rect::from_min_max(rect.min, pos2(split, rect.bottom())),
        CornerRadius {
            nw: radius,
            sw: radius,
            ne: 0,
            se: 0,
        },
        strong,
    );
    let mut fade = egui::Mesh::default();
    fade.colored_vertex(pos2(split, rect.top()), strong);
    fade.colored_vertex(pos2(end, rect.top()), Color32::TRANSPARENT);
    fade.colored_vertex(pos2(end, rect.bottom()), Color32::TRANSPARENT);
    fade.colored_vertex(pos2(split, rect.bottom()), strong);
    fade.add_triangle(0, 1, 2);
    fade.add_triangle(0, 2, 3);
    egui::Shape::Vec(vec![cap, egui::Shape::mesh(fade)])
}

fn card_frame() -> Frame {
    Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
}

/// A card centered in the remaining space (vertically too, using last frame's height).
fn centered_card(ui: &mut egui::Ui, id: &str, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let avail = ui.available_rect_before_wrap();
    let id = egui::Id::unique(id);
    let h: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(420.0);
    let top = ((avail.height() - h) / 2.0).max(8.0);
    let w = width.min(avail.width());
    let rect = Rect::from_min_size(
        pos2(avail.center().x - w / 2.0, avail.top() + top),
        vec2(w, (avail.height() - top).max(0.0)),
    );
    let margin = 24;
    let height = ui
        .scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            card_frame()
                .inner_margin(Margin::same(margin))
                .shadow(ui.visuals().window_shadow)
                .show(ui, |ui| {
                    ui.set_width(w - 2.0 * margin as f32 - 2.0);
                    add(ui);
                })
                .response
                .rect
                .height()
        })
        .inner;
    if (height - h).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, height));
        ui.ctx().request_repaint();
    }
}

fn brand_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
    icons::logo(
        ui.painter(),
        Rect::from_center_size(r.center(), vec2(46.0, 46.0)),
    );
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(title)
                .font(bold_font(24.0))
                .color(theme::TEXT),
        );
        ui.add(egui::Label::new(RichText::new(subtitle).color(theme::TEXT_DIM)).wrap());
    });
}

fn field(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    password: bool,
    hint: &str,
) -> egui::Response {
    ui.label(RichText::new(label).size(12.5).color(theme::TEXT_DIM));
    ui.add_space(-4.0);
    ui.add(
        egui::TextEdit::singleline(value)
            .password(password)
            .hint_text(hint)
            .font(font(14.5))
            .margin(vec2(10.0, 8.0))
            .desired_width(f32::INFINITY),
    )
}

fn enter_pressed(ui: &egui::Ui, r: &egui::Response) -> bool {
    r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// Text in the add field that is not a link is a YouTube search.
fn is_search(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && shared::youtube::parse_youtube_url(t).is_none()
        && !t.starts_with("http://")
        && !t.starts_with("https://")
        && !t.starts_with("www.")
}

/// The filled accent button.
fn primary_button(
    ui: &mut egui::Ui,
    icon: Option<Icon>,
    text: &str,
    size: egui::Vec2,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let fill = if !enabled {
        theme::CARD_HOVER
    } else if resp.is_pointer_button_down_on() {
        theme::accent()
    } else if resp.hovered() {
        theme::accent_hover()
    } else {
        theme::accent()
    };
    let color = if enabled {
        theme::ON_ACCENT
    } else {
        theme::TEXT_FAINT
    };
    let p = ui.painter();
    p.rect_filled(rect, 8.0, fill);
    let g = p.layout_no_wrap(text.to_string(), bold_font(14.5), color);
    let icon_w = if icon.is_some() { 12.0 + 8.0 } else { 0.0 };
    let left = rect.center().x - (icon_w + g.size().x) / 2.0;
    if let Some(icon) = icon {
        let r = Rect::from_min_size(pos2(left, rect.center().y - 6.0), vec2(12.0, 12.0));
        icons::paint(p, icon, r, color);
    }
    p.galley(
        pos2(left + icon_w, rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A small rounded label, optionally with a status dot.
fn tag(
    ui: &mut egui::Ui,
    text: &str,
    fill: Color32,
    stroke: Color32,
    text_color: Color32,
    dot: Option<Color32>,
) -> egui::Response {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font(12.5), text_color);
    let pad_left = if dot.is_some() { 22.0 } else { 10.0 };
    let (rect, resp) =
        ui.allocate_exact_size(vec2(g.size().x + pad_left + 10.0, 24.0), Sense::hover());
    let p = ui.painter();
    p.rect(
        rect,
        12.0,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    if let Some(c) = dot {
        p.circle_filled(pos2(rect.left() + 12.0, rect.center().y), 3.5, c);
    }
    p.galley(
        pos2(rect.left() + pad_left, rect.center().y - g.size().y / 2.0),
        g,
        text_color,
    );
    resp
}

/// A clickable on/off pill (accent when on).
fn toggle_pill(ui: &mut egui::Ui, text: &str, on: bool) -> egui::Response {
    let color = if on {
        theme::accent_hover()
    } else {
        theme::TEXT_DIM
    };
    let g = ui
        .painter()
        .layout_no_wrap(text.to_string(), font(12.5), color);
    let (rect, resp) = ui.allocate_exact_size(vec2(g.size().x + 32.0, 26.0), Sense::click());
    let hovered = resp.hovered();
    let (fill, stroke) = if on {
        (
            theme::accent_alpha(if hovered { 46 } else { 30 }),
            theme::accent(),
        )
    } else if hovered {
        (theme::CARD_HOVER, theme::BORDER)
    } else {
        (Color32::TRANSPARENT, theme::BORDER)
    };
    let p = ui.painter();
    p.rect(
        rect,
        13.0,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    let dot = pos2(rect.left() + 13.0, rect.center().y);
    if on {
        p.circle_filled(dot, 3.5, theme::accent());
    } else {
        p.circle_stroke(dot, 3.0, Stroke::new(1.2, theme::TEXT_FAINT));
    }
    p.galley(
        pos2(rect.left() + 22.0, rect.center().y - g.size().y / 2.0),
        g,
        color,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn error_box(ui: &mut egui::Ui, text: &str) {
    Frame::new()
        .fill(theme::BAD.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, theme::BAD.gamma_multiply(0.4)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(text).color(theme::BAD)).wrap());
        });
}

fn thin_bar(ui: &mut egui::Ui, frac: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 3.0, theme::FIELD);
    if frac > 0.0 {
        let mut done = rect;
        done.set_width(rect.width() * frac.clamp(0.0, 1.0));
        p.rect_filled(done, 3.0, color);
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .font(bold_font(11.0))
            .color(theme::TEXT_FAINT),
    );
}

fn one_line(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w);
    ui.painter().layout_job(job)
}

fn paint_thumb(ui: &egui::Ui, rect: Rect, tex: Option<egui::TextureId>, radius: u8) {
    match tex {
        Some(id) => {
            egui::Image::from_texture((id, rect.size()))
                .corner_radius(CornerRadius::same(radius))
                .paint_at(ui, rect);
        }
        None => {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius), theme::CARD_HOVER);
            let s = rect.height() * 0.42;
            icons::paint(
                ui.painter(),
                Icon::Note,
                Rect::from_center_size(rect.center(), vec2(s, s)),
                theme::TEXT_FAINT,
            );
        }
    }
}

/// Three "equalizer" bars marking the current track.
fn paint_bars(p: &egui::Painter, c: egui::Pos2, playing: bool) {
    let heights: [f32; 3] = if playing {
        [10.0, 16.0, 7.0]
    } else {
        [5.0, 5.0, 5.0]
    };
    for (k, h) in heights.iter().enumerate() {
        let x = c.x - 6.0 + k as f32 * 5.0;
        let r = Rect::from_min_max(pos2(x - 1.5, c.y + 8.0 - h), pos2(x + 1.5, c.y + 8.0));
        p.rect_filled(r, 1.0, theme::accent());
    }
}

fn pending_row(ui: &mut egui::Ui, add: &PendingAdd) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    let p = ui.painter().clone();
    let t = add.started.elapsed().as_secs_f32();
    let pulse = 0.5 + 0.5 * (t * 4.0).sin();
    // Slide/fade in so a new link "lands" in the list.
    let appear = (t / 0.25).min(1.0);
    p.rect_filled(
        rect,
        8.0,
        theme::accent_alpha((8.0 + 14.0 * pulse * appear) as u8),
    );
    let num = Rect::from_min_size(rect.min, vec2(40.0, ROW_H));
    egui::Spinner::new()
        .size(16.0)
        .color(theme::accent())
        .paint_at(ui, Rect::from_center_size(num.center(), vec2(16.0, 16.0)));
    let thumb = Rect::from_min_size(pos2(num.right(), rect.center().y - 18.0), vec2(64.0, 36.0));
    p.rect_filled(
        thumb,
        5.0,
        theme::CARD_HOVER.gamma_multiply(0.6 + 0.4 * pulse),
    );
    let text_left = thumb.right() + 12.0;
    let max_w = (rect.right() - 70.0 - text_left).max(40.0);
    let (title, color) = match &add.title {
        Some(t) => (t.as_str(), theme::TEXT),
        None if add.playlist => (s::LOADING_PLAYLIST, theme::TEXT_DIM),
        None => (s::LOADING_TRACK, theme::TEXT_DIM),
    };
    let g = one_line(ui, title, font(14.0), color, max_w);
    p.galley(
        pos2(text_left, rect.center().y - g.size().y - 1.0),
        g,
        color,
    );
    let g = one_line(ui, s::ADDING, font(12.0), theme::accent(), max_w);
    p.galley(pos2(text_left, rect.center().y + 2.0), g, theme::accent());
    ui.ctx().request_repaint();
}

fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    let avail = ui.available_rect_before_wrap();
    let h = avail.height().min(220.0);
    let rect = Rect::from_min_size(avail.min, vec2(avail.width(), h));
    ui.allocate_rect(rect, Sense::hover());
    let p = ui.painter();
    let c = pos2(rect.center().x, rect.top() + h * 0.38);
    p.circle_filled(c, 26.0, theme::accent_alpha(30));
    icons::paint(
        p,
        Icon::Note,
        Rect::from_center_size(c, vec2(24.0, 24.0)),
        theme::accent(),
    );
    p.text(
        c + vec2(0.0, 42.0),
        Align2::CENTER_CENTER,
        title,
        bold_font(15.0),
        theme::TEXT,
    );
    if !hint.is_empty() {
        let mut job = egui::text::LayoutJob::simple(
            hint.to_string(),
            font(13.0),
            theme::TEXT_DIM,
            (rect.width() - 40.0).max(100.0),
        );
        job.halign = Align::Center;
        let g = p.layout_job(job);
        p.galley(c + vec2(0.0, 58.0), g, theme::TEXT_DIM);
    }
}

fn sidebar_ui(ui: &mut egui::Ui, view: &View) {
    egui::Panel::bottom("sidebar_bottom")
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::NONE)
        .show(ui, |ui| {
            ui.add_space(8.0);
            sync_card(ui, view);
            ui.add_space(6.0);
            about_line(ui, view);
        });
    egui::CentralPanel::default()
        .frame(Frame::NONE)
        .show(ui, |ui| {
            section_label(ui, &s::members_title(view.members.len()));
            ui.add_space(2.0);
            egui::ScrollArea::vertical()
                .id_salt("members_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for m in &view.members {
                        let me = view.me.as_ref().is_some_and(|me| me.id == m.id);
                        let sync = if me { Some(view.sync()) } else { m.sync };
                        member_row(ui, &m.name, me, sync);
                    }
                });
        });
}

/// Where a person's dot sits against the line (points; negative = above)
/// and whether it is a ring (still loading).
fn dot_place(sync: Option<MemberSync>) -> (f32, bool) {
    match sync {
        Some(s) if s.loading => (9.0, true),
        Some(MemberSync {
            drift_ms: Some(d), ..
        }) if (d as i64).abs() > shared::sync::DRIFT_IGNORE_MS => {
            let far = ((d.abs() as f32 - 50.0) / 450.0).clamp(0.0, 1.0);
            let off = 4.0 + 5.0 * far;
            (if d > 0 { -off } else { off }, false)
        }
        _ => (0.0, false),
    }
}

/// "Marko · kasni 120 ms". Just the name when the server doesn't say.
fn dot_text(name: &str, sync: Option<MemberSync>, playing: bool) -> String {
    let state = match sync {
        None => return name.to_string(),
        Some(s) if s.loading => s::DOT_LOADING.to_string(),
        Some(MemberSync {
            drift_ms: Some(d), ..
        }) if (d as i64).abs() > shared::sync::DRIFT_IGNORE_MS => s::dot_drift(d),
        Some(MemberSync {
            drift_ms: Some(_), ..
        }) => s::DOT_SYNCED.to_string(),
        Some(_) if playing => s::DOT_WAITING.to_string(),
        Some(_) => return name.to_string(),
    };
    format!("{name} · {state}")
}

/// One dot per person on a shared line. On the line: in sync. Off it:
/// ahead (above), behind (below) or still loading (a ring hanging below).
/// When everyone is on it while a song plays, the line lights up.
fn sync_dots(ui: &mut egui::Ui, view: &View) {
    const H: f32 = 30.0;
    const STEP: f32 = 16.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), H), Sense::hover());
    let n = view.members.len();
    if n == 0 {
        return;
    }
    let step = STEP.min((rect.width() - 12.0) / n as f32);
    let x0 = rect.center().x - step * (n - 1) as f32 / 2.0;
    let y = rect.center().y;
    let playing = matches!(view.phase, Phase::Playing);
    let dots: Vec<_> = view
        .members
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let me = view.me.as_ref().is_some_and(|me| me.id == m.id);
            let sync = if me { Some(view.sync()) } else { m.sync };
            (x0 + step * i as f32, m, me, sync)
        })
        .collect();
    let together = playing && dots.iter().all(|d| dot_place(d.3) == (0.0, false));
    let lit = ui
        .ctx()
        .animate_bool_with_time(Id::unique("sync_line"), together, 0.25);
    let p = ui.painter_at(rect);
    p.hline(
        rect.x_range(),
        y,
        Stroke::new(
            1.0 + 0.5 * lit,
            theme::mix(theme::BORDER, theme::GOOD, 0.8 * lit),
        ),
    );
    for (x, m, me, sync) in &dots {
        let (dy, ring) = dot_place(*sync);
        let dy = ui
            .ctx()
            .animate_value_with_time(Id::unique(("sync_dot", m.id)), dy, 0.22);
        let c = pos2(*x, y + dy);
        let r = if *me { 4.5 } else { 3.5 };
        let color = theme::name_color(&m.name);
        // A gap in the line around each dot.
        p.circle_filled(c, r + 2.0, theme::CARD);
        if ring {
            p.circle_stroke(c, r - 0.75, Stroke::new(1.5, color));
        } else {
            p.circle_filled(c, r, color);
        }
    }
    if let Some(pos) = resp.hover_pos() {
        let near = dots
            .iter()
            .filter(|d| (d.0 - pos.x).abs() <= (step / 2.0).max(5.0))
            .min_by(|a, b| (a.0 - pos.x).abs().total_cmp(&(b.0 - pos.x).abs()));
        let text = match near {
            Some((_, m, _, sync)) => dot_text(&m.name, *sync, playing),
            None => status(view).0,
        };
        resp.on_hover_text_at_pointer(text);
    }
}

fn member_row(ui: &mut egui::Ui, name: &str, me: bool, sync: Option<MemberSync>) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
        let c = theme::name_color(name);
        let p = ui.painter();
        p.circle_filled(r.center(), 15.0, c.gamma_multiply(0.22));
        let initial: String = name
            .chars()
            .next()
            .map(|ch| ch.to_uppercase().collect())
            .unwrap_or_default();
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            initial,
            bold_font(13.0),
            c,
        );
        p.circle_filled(r.right_bottom() - vec2(3.0, 3.0), 4.5, theme::PANEL);
        let state = if dot_place(sync) == (0.0, false) {
            theme::GOOD
        } else {
            theme::WARN
        };
        p.circle_filled(r.right_bottom() - vec2(3.0, 3.0), 3.0, state);
        let text = RichText::new(name).color(theme::TEXT);
        ui.add(egui::Label::new(if me { text.strong() } else { text }).truncate());
        if me {
            ui.label(RichText::new(s::YOU).size(12.0).color(theme::TEXT_FAINT));
        }
    });
}

fn sync_card(ui: &mut egui::Ui, view: &View) {
    Frame::new()
        .fill(theme::CARD)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 4.0;
            section_label(ui, s::SYNC_TITLE);
            sync_dots(ui, view);
            // While a song plays the dots say it (the words are their
            // tooltip); every other state still gets its words.
            if !matches!(view.phase, Phase::Playing) {
                let (status_text, status_color) = status(view);
                kv(ui, s::STATUS, &status_text, status_color);
            }
            kv(
                ui,
                s::PING,
                &view.rtt_ms.map_or("—".into(), |r| format!("{r} ms")),
                theme::TEXT,
            );
            kv(
                ui,
                s::DRIFT,
                &match (&view.phase, view.drift_ms) {
                    (Phase::Playing, Some(d)) => s::drift(d),
                    _ => "—".into(),
                },
                theme::TEXT,
            );
            let (clock, color) = if view.clock_synced {
                (s::CLOCK_OK, theme::GOOD)
            } else {
                (s::CLOCK_SYNCING, theme::WARN)
            };
            kv(ui, s::CLOCK, clock, color);
        });
}

fn kv(ui: &mut egui::Ui, k: &str, v: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(k).size(12.5).color(theme::TEXT_DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(v).size(12.5).color(color)).truncate());
        });
    });
}

/// Player/sync status text and color.
fn status(view: &View) -> (String, Color32) {
    match &view.phase {
        Phase::Idle => match view.room.as_ref().map(|r| r.playback.status) {
            Some(PlayStatus::Stopped) | None => (s::PHASE_STOPPED.into(), theme::TEXT_FAINT),
            _ => (s::IDLE.into(), theme::TEXT_DIM),
        },
        Phase::Resolving => (s::PHASE_RESOLVING.into(), theme::accent()),
        Phase::Buffering => (s::PHASE_BUFFERING.into(), theme::accent()),
        Phase::Waiting => {
            if view.room.as_ref().is_some_and(|r| r.waiting_for_ready) {
                (s::PHASE_WAITING.into(), theme::WARN)
            } else {
                (s::PHASE_STARTING.into(), theme::accent())
            }
        }
        Phase::Paused => (s::PHASE_PAUSED.into(), theme::TEXT_DIM),
        Phase::Playing => match view.drift_ms {
            Some(d) if d.abs() > shared::sync::DRIFT_IGNORE_MS => (s::SYNCING.into(), theme::WARN),
            _ => (s::SYNCED.into(), theme::GOOD),
        },
        Phase::Error(e) => (e.clone(), theme::BAD),
    }
}

fn connection_pill(ui: &mut egui::Ui, view: &View) {
    let (color, text) = match &view.conn {
        ConnState::Connected => (
            theme::GOOD,
            match view.rtt_ms {
                Some(r) => format!("{} · {r} ms", s::CONNECTED),
                None => s::CONNECTED.to_string(),
            },
        ),
        ConnState::Connecting { waking: true } => (theme::WARN, s::WAKING_SERVER.to_string()),
        ConnState::Connecting { .. } => (theme::WARN, s::CONNECTING.to_string()),
        ConnState::Reconnecting { retry_in_s } => (theme::WARN, s::reconnecting_in(*retry_in_s)),
        _ => (theme::BAD, s::OFFLINE.to_string()),
    };
    tag(
        ui,
        &text,
        color.gamma_multiply(0.1),
        color.gamma_multiply(0.35),
        theme::TEXT,
        Some(color),
    );
}

fn footer(ui: &egui::Ui, view: &View) {
    let rect = ui.max_rect();
    ui.painter().text(
        pos2(rect.center().x, rect.bottom()),
        Align2::CENTER_BOTTOM,
        about_text(view),
        font(11.5),
        theme::TEXT_FAINT,
    );
}

fn about_text(view: &View) -> String {
    let ytdlp = if view.ytdlp_updating {
        s::YTDLP_UPDATING.to_string()
    } else {
        view.ytdlp_version.clone().unwrap_or_else(|| "—".into())
    };
    let mut text = format!(
        "{} {} · yt-dlp {}",
        s::APP_TITLE,
        env!("CARGO_PKG_VERSION"),
        ytdlp
    );
    if view.fake_audio {
        text.push_str(" · ");
        text.push_str(s::FAKE_AUDIO);
    }
    text
}

fn about_line(ui: &mut egui::Ui, view: &View) {
    ui.add(
        egui::Label::new(
            RichText::new(about_text(view))
                .size(11.0)
                .color(theme::TEXT_FAINT),
        )
        .wrap(),
    )
    .on_hover_text(format!("{} · {}", s::ABOUT, s::YTDLP_VERSION));
}

/// `m:ss` or `h:mm:ss`.
pub fn fmt_time(ms: u64) -> String {
    let total = ms / 1000;
    let (h, m, sec) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let view = self.view();
        let ctx = ui.ctx().clone();
        self.thumbs.begin_frame(&ctx);
        self.update_accent(&ctx, &view);
        self.video_rect = None;
        self.video_cut = 0.0;
        if view.video_closed != self.video_closed_seen {
            // The user closed the video window: video mode is off now.
            self.video_closed_seen = view.video_closed;
            if self.config.video {
                self.config.video = false;
                self.config.save();
            }
        }
        match &self.screen {
            Screen::Deps(shared) => {
                let shared = shared.clone();
                self.deps_ui(ui, &view, shared);
            }
            Screen::Connect => self.connect_ui(ui, &view),
            Screen::Room => {
                self.room_ui(ui, &view);
                if self.settings_open && matches!(self.screen, Screen::Room) {
                    self.settings_ui(&ctx);
                }
            }
        }
        if !matches!(self.screen, Screen::Room) && self.video_full {
            self.set_video_full(&ctx, false);
        }
        self.toasts_ui(&ctx, &view);
        // The native video area covers egui drawing: hide it under menus
        // and windows.
        let covered = self.settings_open || egui::Popup::is_any_open(&ctx);
        let rect = self.video_rect.filter(|_| !covered);
        if let Some(h) = &mut self.video_host {
            h.place(rect, self.video_cut, ctx.pixels_per_point());
        }
        // Animate the progress bar only while something plays, about one
        // repaint per pixel of progress (bounded).
        if matches!(self.screen, Screen::Room) && view.is_playing() {
            let d = view
                .room
                .as_ref()
                .and_then(|r| r.current_track())
                .and_then(|t| t.duration_ms)
                .unwrap_or(0);
            let ms_per_px = d as f32 / self.bar_width.max(50.0);
            ctx.request_repaint_after(Duration::from_millis(ms_per_px.clamp(33.0, 250.0) as u64));
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.config.volume = self.volume;
        self.config.save();
        if let Some(e) = self.engine.take() {
            e.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_time;
    use crate::strings as s;

    /// Every symbol the UI uses must exist in the bundled fonts.
    #[test]
    fn glyphs_exist() {
        use eframe::egui;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let all: String = [
            s::APP_TITLE,
            s::CONNECT_HEADING,
            s::RECENT_ROOMS,
            s::PLAY_THIS,
            s::SEARCH,
            s::SEARCH_TITLE,
            s::BACK_TO_QUEUE,
            s::ADD_TO_QUEUE,
            s::IN_QUEUE,
            s::SEARCHING,
            s::SEARCH_NOTHING,
            s::SEARCH_FAILED,
            s::SEARCH_HINT,
            s::SEARCH_SONGS,
            s::SEARCH_PLAYLISTS,
            s::PLAYLIST_TAG,
            s::SEARCH_HINT_PLAYLISTS,
            s::ADD_PLAYLIST,
            s::SETTINGS,
            s::CLOSE,
            s::EQ_TITLE,
            s::EQ_HINT,
            s::EQ_RESET,
            s::EQ_CUSTOM,
            s::VIDEO_TITLE,
            s::VIDEO_EXPLAIN,
            s::VIDEO_FULL_HINT,
            s::EXIT_FULLSCREEN,
            s::ON,
            s::OFF,
            "Ravno Bas Rok Pop Vokal Elektronika +12 dB −12 32 64 125 250 500 1k 2k 4k 8k 16k",
            s::NORMALIZE,
            s::NORMALIZE_ON,
            s::NORMALIZE_OFF,
            s::RECENT_JOIN,
            s::RECENT_FORGET,
            s::WAKING_SERVER,
            s::RECONNECTING,
            s::SERVER_UNREACHABLE,
            s::NOTHING_PLAYING,
            s::NOTHING_PLAYING_HINT,
            s::PASTE_LINK,
            s::PASTE_HINT,
            s::ADDING,
            s::PHASE_RESOLVING,
            s::PHASE_BUFFERING,
            s::PHASE_WAITING,
            s::RESOLVE_FAILED,
            s::SKIPPING_BROKEN,
            s::DEPS_HEADING,
            s::DEPS_EXPLAIN,
            s::YTDLP_UPDATING,
            s::SERVER_URL_HINT,
            s::MOVE_UP,
            s::PLAY_THIS,
            s::SEARCH,
            s::SEARCH_TITLE,
            s::BACK_TO_QUEUE,
            s::ADD_TO_QUEUE,
            s::IN_QUEUE,
            s::SEARCHING,
            s::SEARCH_NOTHING,
            s::SEARCH_FAILED,
            s::SEARCH_HINT,
            s::ADDED_BY,
            s::SEARCH_QUEUE,
            s::LOADING_PLAYLIST,
            s::QUEUE_EMPTY_HINT,
            s::CLOCK_OK,
            s::DOT_SYNCED,
            s::DOT_LOADING,
            s::DOT_WAITING,
            &s::dot_drift(-120),
            &s::dot_drift(120),
            s::DOUBLE_CLICK_HINT,
            &s::NOW_PLAYING.to_uppercase(),
            &s::SYNC_TITLE.to_uppercase(),
            "čćšžđČĆŠŽĐ…·—+%",
        ]
        .concat();
        for family in [egui::FontFamily::Proportional, crate::theme::bold()] {
            let missing: String = ctx.fonts_mut(|f| {
                all.chars()
                    .filter(|c| !c.is_whitespace())
                    .filter(|c| !f.has_glyph(&egui::FontId::new(14.0, family.clone()), *c))
                    .collect()
            });
            assert!(
                missing.is_empty(),
                "missing glyphs in {family:?}: {missing}"
            );
        }
    }

    #[test]
    fn formats_time() {
        assert_eq!(fmt_time(0), "0:00");
        assert_eq!(fmt_time(61_500), "1:01");
        assert_eq!(fmt_time(3_725_000), "1:02:05");
    }

    #[test]
    fn serbian_plurals() {
        assert_eq!(s::songs(1), "1 pesma");
        assert_eq!(s::songs(3), "3 pesme");
        assert_eq!(s::songs(5), "5 pesama");
        assert_eq!(s::songs(11), "11 pesama");
        assert_eq!(s::songs(12), "12 pesama");
        assert_eq!(s::songs(21), "21 pesma");
        assert_eq!(s::songs(24), "24 pesme");
    }
}

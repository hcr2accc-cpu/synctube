//! egui front-end: dependency download, connect screen and room screen.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, Event, FontId, Frame, Key, Layout,
    Margin, Rect, RichText, Sense, Stroke, StrokeKind, UiBuilder,
};
use shared::{PlayStatus, RoomState};
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

const TOAST_SECS: f32 = 5.0;
const TOAST_FADE: f32 = 0.35;
const ROW_H: f32 = 52.0;
const ROW_GAP: f32 = 2.0;
const SIDEBAR_W: f32 = 236.0;
/// Keep showing a seek target until the player catches up (avoids the
/// progress bar jumping back for a moment).
const SEEK_HOLD: Duration = Duration::from_millis(1500);

enum Screen {
    Deps(deps::Shared),
    Connect,
    Room,
}

enum Scroll {
    Index(usize),
    End,
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
    seek_drag: Option<f32>,
    seek_hold: Option<(u64, Instant)>,
    bar_width: f32,
    notify: Arc<dyn Fn() + Send + Sync>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, fake_audio: bool) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install(&ctx);
        let thumbs = Thumbs::new(ctx.clone());
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let config = Config::load();
        let tools = Tools::discover();
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
            },
            notify.clone(),
        );
        Self {
            engine: Some(engine),
            volume: config.volume,
            config,
            screen,
            connecting: false,
            connect_error: None,
            link: String::new(),
            focus_link: false,
            filter: String::new(),
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
                                    ui.add(egui::Spinner::new().size(13.0).color(theme::ACCENT));
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
                        _ => theme::ACCENT,
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
                        ui.add(egui::Spinner::new().size(16.0).color(theme::ACCENT));
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
                    ui.add(egui::Spinner::new().size(28.0).color(theme::ACCENT));
                });
            });
            return;
        };

        self.global_keys(ui, &room);

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
                            self.screen = Screen::Connect;
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
                self.now_playing_ui(ui, view, &room);
                ui.add_space(14.0);
                self.queue_ui(ui, view, &room);
            });
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
            let enter = enter_pressed(ui, &r);
            let h = r.rect.height().max(36.0);
            let clicked =
                primary_button(ui, Some(Icon::Plus), s::ADD, vec2(btn_w, h), can_add).clicked();
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
            let link = std::mem::take(&mut self.link);
            self.send(Command::Add(link.trim().to_string()));
            self.scroll = Some(Scroll::End);
        }
    }

    fn now_playing_ui(&mut self, ui: &mut egui::Ui, view: &View, room: &RoomState) {
        let track = room.current_track().cloned();
        card_frame().show(ui, |ui| {
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
                            theme::ACCENT,
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
                            .color(theme::ACCENT),
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
    }

    fn progress_ui(&mut self, ui: &mut egui::Ui, view: &View, track: Option<&shared::Track>) {
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
            p.rect_filled(done, h / 2.0, theme::ACCENT);
        }
        if active {
            p.circle_filled(pos2(done.right(), bar.center().y), 7.0, Color32::WHITE);
            if let (Some(f), Some(d)) = (pointer_frac, duration) {
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

    fn transport_ui(&mut self, ui: &mut egui::Ui, room: &RoomState, has_track: bool) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            if icons::ghost(ui, Icon::Previous, 36.0, has_track)
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
            if icons::round(ui, icon, 46.0, has_track || !room.queue.is_empty())
                .on_hover_text(tip)
                .clicked()
            {
                self.send(Command::TogglePlay);
            }
            if icons::ghost(ui, Icon::Next, 36.0, has_track)
                .on_hover_text(s::NEXT)
                .clicked()
            {
                self.send(Command::Next);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_sized(
                    [36.0, 20.0],
                    egui::Label::new(
                        RichText::new(format!("{:.0}%", self.volume))
                            .size(12.5)
                            .color(theme::TEXT_DIM),
                    ),
                );
                ui.spacing_mut().slider_width = 110.0;
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
            return;
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
            for r in range {
                if let Some(&i) = rows.get(r) {
                    self.queue_row(ui, room, i, current, playing);
                } else if let Some(p) = pending.get(r - rows.len()) {
                    pending_row(ui, p);
                }
            }
        });
    }

    fn queue_row(
        &mut self,
        ui: &mut egui::Ui,
        room: &RoomState,
        i: usize,
        current: Option<usize>,
        playing: bool,
    ) {
        let t = &room.queue[i];
        let len = room.queue.len();
        let is_cur = current == Some(i);
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let p = ui.painter().clone();
        if is_cur {
            p.rect_filled(rect, 8.0, theme::accent_alpha(30));
            p.rect_filled(
                Rect::from_min_size(rect.min + vec2(0.0, 12.0), vec2(3.0, ROW_H - 24.0)),
                2.0,
                theme::ACCENT,
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
                        let color = if t.error { theme::BAD } else { theme::ACCENT };
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

// ----- shared widgets ----------------------------------------------------------------

fn background_panel() -> egui::CentralPanel {
    egui::CentralPanel::default().frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(16)))
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
        theme::ACCENT
    } else if resp.hovered() {
        theme::ACCENT_HOVER
    } else {
        theme::ACCENT
    };
    let color = if enabled {
        Color32::WHITE
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
        theme::ACCENT_HOVER
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
            theme::ACCENT,
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
        p.circle_filled(dot, 3.5, theme::ACCENT);
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
        p.rect_filled(r, 1.0, theme::ACCENT);
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
        .color(theme::ACCENT)
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
    let g = one_line(ui, s::ADDING, font(12.0), theme::ACCENT, max_w);
    p.galley(pos2(text_left, rect.center().y + 2.0), g, theme::ACCENT);
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
        theme::ACCENT,
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
                        member_row(ui, &m.name, me);
                    }
                });
        });
}

fn member_row(ui: &mut egui::Ui, name: &str, me: bool) {
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
        p.circle_filled(r.right_bottom() - vec2(3.0, 3.0), 3.0, theme::GOOD);
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
            let (status_text, status_color) = status(view);
            kv(ui, s::STATUS, &status_text, status_color);
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
        Phase::Resolving => (s::PHASE_RESOLVING.into(), theme::ACCENT),
        Phase::Buffering => (s::PHASE_BUFFERING.into(), theme::ACCENT),
        Phase::Waiting => {
            if view.room.as_ref().is_some_and(|r| r.waiting_for_ready) {
                (s::PHASE_WAITING.into(), theme::WARN)
            } else {
                (s::PHASE_STARTING.into(), theme::ACCENT)
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
        match &self.screen {
            Screen::Deps(shared) => {
                let shared = shared.clone();
                self.deps_ui(ui, &view, shared);
            }
            Screen::Connect => self.connect_ui(ui, &view),
            Screen::Room => self.room_ui(ui, &view),
        }
        self.toasts_ui(&ctx, &view);
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
            s::ADDED_BY,
            s::SEARCH_QUEUE,
            s::LOADING_PLAYLIST,
            s::QUEUE_EMPTY_HINT,
            s::CLOCK_OK,
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

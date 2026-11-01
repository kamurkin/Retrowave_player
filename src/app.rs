use crate::media::{MediaType, Track, format_time};
use crate::playback::{
    PlayRequestPhase, Playback, PlaybackSnapshot, PlaybackState, TrackMetadata, VideoPlayResult,
};
use crate::playlist::Playlist;
use crate::skin;
use eframe::egui::{
    self, Align2, Color32, FontId, Frame, Margin, RichText, Stroke, ViewportBuilder, ViewportId,
    pos2, vec2,
};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const VIDEO_TITLE: &str = "RetroWave Video";
static DIAGNOSTIC_EVENT_SEQ: AtomicU64 = AtomicU64::new(0);
static DIAGNOSTIC_START: OnceLock<Instant> = OnceLock::new();
const BANDS: [&str; 10] = [
    "60", "170", "310", "600", "1K", "3K", "6K", "12K", "14K", "16K",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PauseAction {
    Play,
    Pause(Option<u64>),
}

fn natural_end_next_index(current: Option<usize>, track_count: usize) -> Option<usize> {
    let current = current?;
    let next = current.checked_add(1)?;
    (next < track_count).then_some(next)
}

fn pause_action(snapshot: &PlaybackSnapshot) -> PauseAction {
    match snapshot.play_request_phase {
        PlayRequestPhase::PendingDelay(request_id) | PlayRequestPhase::Loading(request_id) => {
            PauseAction::Pause(Some(request_id))
        }
        PlayRequestPhase::Active(_) | PlayRequestPhase::None => {
            if snapshot.path.is_none() || snapshot.state == PlaybackState::Stopped {
                PauseAction::Play
            } else {
                PauseAction::Pause(None)
            }
        }
    }
}

fn playlist_viewport() -> ViewportId {
    ViewportId::from_hash_of("retrowave-playlist")
}

fn equalizer_viewport() -> ViewportId {
    ViewportId::from_hash_of("retrowave-equalizer")
}

fn video_viewport() -> ViewportId {
    ViewportId::from_hash_of("retrowave-video")
}

#[derive(Clone, Debug, PartialEq)]
struct ViewportDiagnosticState {
    minimized: Option<bool>,
    visible: Option<bool>,
    focused: Option<bool>,
    fullscreen: Option<bool>,
}

impl From<&egui::ViewportInfo> for ViewportDiagnosticState {
    fn from(info: &egui::ViewportInfo) -> Self {
        Self {
            minimized: info.minimized,
            visible: info.visible(),
            focused: info.focused,
            fullscreen: info.fullscreen,
        }
    }
}

fn log_diagnostic_record(category: &str, root_call_seq: u64, message: impl std::fmt::Display) {
    let started_at = DIAGNOSTIC_START.get_or_init(Instant::now);
    let event_seq = DIAGNOSTIC_EVENT_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    eprintln!(
        "[{category}] event_seq={event_seq} mono_us={} root_call_seq={root_call_seq} {message}",
        started_at.elapsed().as_micros()
    );
}

fn log_egui_record(
    context: &egui::Context,
    viewport_name: &str,
    root_call_seq: u64,
    event: &str,
    details: impl std::fmt::Display,
) {
    let viewport_id = context.viewport_id();
    log_diagnostic_record(
        "viewport-diag",
        root_call_seq,
        format_args!(
            "viewport={viewport_name} viewport_id={viewport_id:?} \
             cumulative_frame_nr={} cumulative_pass_nr={} current_pass_index={} \
             event={event} {details}",
            context.cumulative_frame_nr_for(viewport_id),
            context.cumulative_pass_nr_for(viewport_id),
            context.current_pass_index(),
        ),
    );
}

fn update_diagnostic_sample(
    current: ViewportDiagnosticState,
    previous: &mut Option<ViewportDiagnosticState>,
    restore_frames_remaining: &mut u8,
) -> (bool, &'static str) {
    let old = previous.replace(current.clone());
    let baseline = old.is_none();
    let changed = old.as_ref().is_some_and(|old| old != &current);
    let restored = old.as_ref().is_some_and(|old| {
        (old.minimized == Some(true) && current.minimized == Some(false))
            || (old.visible == Some(false) && current.visible == Some(true))
    });

    if restored {
        *restore_frames_remaining = 3;
    }
    let post_restore = !restored && *restore_frames_remaining > 0;
    let sample = baseline || changed || post_restore;
    let phase = if baseline {
        "baseline"
    } else if restored {
        "restored"
    } else if changed {
        if current.minimized == Some(true) || current.visible == Some(false) {
            "minimized-or-not-render-visible"
        } else {
            "state-change"
        }
    } else if post_restore {
        "post-restore"
    } else {
        "unchanged"
    };

    if post_restore {
        *restore_frames_remaining -= 1;
    }
    (sample, phase)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VideoWindowDiagnosticState {
    parent_hwnd: usize,
    child_hwnd: usize,
    viewport_rect: egui::Rect,
    video_rect: egui::Rect,
    pixels_per_point: f32,
    set_window_pos: (i32, i32, i32, i32),
    parent_minimized: bool,
    parent_visible: bool,
    child_visible: bool,
    video_open: bool,
    video_visible: bool,
}

impl VideoWindowDiagnosticState {
    fn same_lifecycle_state(&self, other: &Self) -> bool {
        self.parent_hwnd == other.parent_hwnd
            && self.child_hwnd == other.child_hwnd
            && self.parent_minimized == other.parent_minimized
            && self.parent_visible == other.parent_visible
            && self.child_visible == other.child_visible
            && self.video_open == other.video_open
            && self.video_visible == other.video_visible
    }
}

pub struct RetroWaveApp {
    playback: Playback,
    snapshot: PlaybackSnapshot,
    playlist: Playlist,
    playlist_open: bool,
    equalizer_open: bool,
    video_open: bool,
    video_container_created: bool,
    video_visible: bool,
    video_fullscreen: bool,
    video_output_hwnd: usize,
    last_video_window_diagnostic: Option<VideoWindowDiagnosticState>,
    last_video_window_diagnostic_at: Option<Instant>,
    diagnostic_root_call_seq: u64,
    last_root_diagnostic_state: Option<ViewportDiagnosticState>,
    root_restore_diagnostic_frames_remaining: u8,
    last_root_video_diagnostic_state: Option<ViewportDiagnosticState>,
    root_video_restore_diagnostic_frames_remaining: u8,
    last_video_diagnostic_state: Option<ViewportDiagnosticState>,
    video_restore_diagnostic_frames_remaining: u8,
    video_title_drag_continuation_logged: bool,
    video_resize_drag_index: Option<usize>,
    video_resize_drag_continuation_logged: bool,
    pending_video_start: Option<(PathBuf, u64)>,
    volume: u8,
    muted: bool,
    rate: f32,
    equalizer_enabled: bool,
    preamp: f32,
    bands: [f32; 10],
    seek_value: f64,
    seek_dragging: bool,
    notice: Option<String>,
    next_play_request_id: u64,
    video_request_id: Option<u64>,
    handled_natural_end_event_id: Option<u64>,
    manual_stop_pending: bool,
}

impl RetroWaveApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        DIAGNOSTIC_START.get_or_init(Instant::now);
        skin::apply(&context.egui_ctx);
        Self {
            playback: Playback::start(),
            snapshot: PlaybackSnapshot::default(),
            playlist: Playlist::default(),
            playlist_open: false,
            equalizer_open: false,
            video_open: false,
            video_container_created: false,
            video_visible: false,
            video_fullscreen: false,
            video_output_hwnd: 0,
            last_video_window_diagnostic: None,
            last_video_window_diagnostic_at: None,
            diagnostic_root_call_seq: 0,
            last_root_diagnostic_state: None,
            root_restore_diagnostic_frames_remaining: 0,
            last_root_video_diagnostic_state: None,
            root_video_restore_diagnostic_frames_remaining: 0,
            last_video_diagnostic_state: None,
            video_restore_diagnostic_frames_remaining: 0,
            video_title_drag_continuation_logged: false,
            video_resize_drag_index: None,
            video_resize_drag_continuation_logged: false,
            pending_video_start: None,
            volume: 70,
            muted: false,
            rate: 1.0,
            equalizer_enabled: false,
            preamp: 0.0,
            bands: [0.0; 10],
            seek_value: 0.0,
            seek_dragging: false,
            notice: None,
            next_play_request_id: 0,
            video_request_id: None,
            handled_natural_end_event_id: None,
            manual_stop_pending: false,
        }
    }

    fn add_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let (added, errors) = self.playlist.add_paths(paths);
        if !added.is_empty() {
            if self.playlist.selected.is_none() {
                self.playlist.selected = Some(self.playlist.tracks.len() - 1);
            }
            self.playback.probe(added);
        }
        if !errors.is_empty() {
            self.notice = Some(errors.join("\n"));
        }
    }

    fn choose_files(&mut self) {
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(
                "Supported media",
                &["mp3", "flac", "wav", "mp4", "mkv", "avi"],
            )
            .pick_files()
        {
            self.add_paths(paths);
        }
    }

    fn play_index(&mut self, index: usize) {
        let Some(track) = self.playlist.tracks.get(index) else {
            self.notice = Some("Select a track before pressing Play.".to_owned());
            return;
        };
        let path = track.path.clone();
        let is_video = track.media_type == MediaType::Video;
        if self.snapshot.path.as_ref() == Some(&path) {
            match self.snapshot.state {
                PlaybackState::Paused => {
                    self.playlist.current = Some(index);
                    self.playlist.selected = Some(index);
                    self.notice = None;
                    self.playback.pause();
                    return;
                }
                PlaybackState::Playing | PlaybackState::Loading => return,
                PlaybackState::Stopped => {}
            }
        }
        self.playlist.selected = Some(index);
        self.notice = None;
        self.next_play_request_id = self.next_play_request_id.wrapping_add(1);
        let request_id = self.next_play_request_id;
        if is_video {
            self.video_request_id = Some(request_id);
            self.pending_video_start = Some((path, request_id));
            self.video_open = true;
            if !self.video_container_created {
                self.video_visible = false;
                self.video_container_created = true;
            }
        } else {
            self.close_video();
            self.playback.play(path, 0, request_id, false);
        }
    }

    fn play_selected(&mut self) {
        let index = self
            .playlist
            .selected
            .or(self.playlist.current)
            .unwrap_or(0);
        self.play_index(index);
    }

    fn stop_playback(&mut self) {
        self.manual_stop_pending = true;
        self.playback.stop();
    }

    fn close_video(&mut self) {
        reset_video_viewport_state(
            &mut self.video_open,
            &mut self.video_visible,
            &mut self.video_fullscreen,
            &mut self.pending_video_start,
            &mut self.video_request_id,
        );
        self.playback.set_video_target(0);
        if self.video_output_hwnd != 0 {
            set_video_output_visible(self.video_output_hwnd, false);
        }
    }

    fn hide_video(&mut self, context: &egui::Context) {
        self.video_visible = false;
        context.send_viewport_cmd_to(video_viewport(), egui::ViewportCommand::Visible(false));
    }

    fn play_next(&mut self, offset: isize) {
        if self.playlist.tracks.is_empty() {
            self.notice = Some("Add a media file to the playlist first.".to_owned());
            return;
        }
        let current = self.playlist.current.unwrap_or(0) as isize;
        let count = self.playlist.tracks.len() as isize;
        let index = (current + offset).rem_euclid(count) as usize;
        self.play_index(index);
    }

    fn update_metadata(&mut self) {
        self.playlist.current = playlist_current_for_path(
            &self.playlist.tracks,
            self.playlist.current,
            self.snapshot.path.as_ref(),
        );
        self.consume_video_play_result();
        for track in &mut self.playlist.tracks {
            let Some(metadata) = self.snapshot.metadata.get(&track.path) else {
                continue;
            };
            apply_metadata(track, metadata);
        }
        if self.snapshot.path.is_some() {
            self.volume = self.snapshot.volume;
            self.muted = self.snapshot.muted;
            self.rate = self.snapshot.rate;
        }
    }

    fn consume_natural_end(&mut self) {
        let Some(event) = self.snapshot.natural_end_event.clone() else {
            if self.manual_stop_pending && self.snapshot.state == PlaybackState::Stopped {
                self.manual_stop_pending = false;
            }
            return;
        };
        if self.handled_natural_end_event_id == Some(event.event_id) {
            return;
        }
        if self.manual_stop_pending {
            self.handled_natural_end_event_id = Some(event.event_id);
            self.playback.acknowledge_natural_end(&event);
            return;
        }
        match self.snapshot.play_request_phase {
            PlayRequestPhase::PendingDelay(request_id)
            | PlayRequestPhase::Loading(request_id)
            | PlayRequestPhase::Active(request_id) => {
                if request_id != event.request_id {
                    return;
                }
            }
            PlayRequestPhase::None => {}
        }

        self.handled_natural_end_event_id = Some(event.event_id);
        self.playback.acknowledge_natural_end(&event);

        if let Some(next) =
            natural_end_next_index(self.playlist.current, self.playlist.tracks.len())
        {
            self.play_index(next);
        } else {
            self.stop_playback();
            self.close_video();
        }
    }

    fn consume_video_play_result(&mut self) {
        let Some(result) = self.snapshot.video_play_result.clone() else {
            return;
        };
        let Some(request_id) = self.video_request_id else {
            return;
        };
        if confirmed_video_path(request_id, &result).is_some() {
            self.video_request_id = None;
            self.video_open = true;
            self.video_visible = true;
        }
    }

    fn show_player(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(4.0, 1.0);
        let title = skin::title_bar(
            ui,
            "RETROWAVE PLAYER",
            &[
                ("PL", self.playlist_open),
                ("EQ", self.equalizer_open),
                ("_", false),
                ("X", false),
            ],
        );
        if title.drag_started {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        match title.clicked {
            Some(0) => self.playlist_open = !self.playlist_open,
            Some(1) => self.equalizer_open = !self.equalizer_open,
            Some(2) => ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
            Some(3) => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
            _ => {}
        }

        Frame::new()
            .fill(skin::PANEL_BG)
            .inner_margin(Margin::same(5))
            .show(ui, |ui| {
                self.show_lcd(ui);
                ui.add_space(1.0);
                self.show_seek(ui);
                ui.add_space(1.0);
                self.show_transport(ui);
                self.show_playback_options(ui);
                let status = if let Some(error) = &self.snapshot.error {
                    Some((error.as_str(), skin::LCD_RED, 8.0))
                } else if let Some(notice) = &self.notice {
                    Some((notice.as_str(), skin::LCD_YELLOW, 8.0))
                } else if let Some(warning) = &self.snapshot.history_warning {
                    Some((warning.as_str(), skin::LCD_YELLOW, 8.0))
                } else if !self.snapshot.ready {
                    Some(("INITIALIZING LIBVLC", skin::LCD_DIM, 7.0))
                } else {
                    None
                };
                let (_, status_rect) = ui.allocate_space(vec2(ui.available_width(), 12.0));
                if let Some((text, color, size)) = status {
                    let display_text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    let response = ui.put(
                        status_rect,
                        egui::Label::new(RichText::new(display_text).color(color).size(size))
                            .truncate(),
                    );
                    response.on_hover_text(text);
                }
            });
    }

    fn show_lcd(&self, ui: &mut egui::Ui) {
        let rect = skin::lcd_panel(ui, vec2(ui.available_width(), 46.0));
        let painter = ui.painter().with_clip_rect(rect.shrink(4.0));
        let current = self
            .playlist
            .current
            .and_then(|index| self.playlist.tracks.get(index));
        let title = current.map_or("NO TRACK LOADED", |track| track.title.as_str());
        let artist = current
            .and_then(|track| track.artist.as_deref())
            .or_else(|| current.and_then(|track| track.album.as_deref()))
            .unwrap_or("RETROWAVE PLAYER");
        painter.text(
            pos2(rect.left() + 6.0, rect.top() + 15.0),
            Align2::LEFT_CENTER,
            format_time(self.snapshot.position),
            FontId::monospace(20.0),
            skin::LCD_GREEN,
        );
        painter.text(
            pos2(rect.left() + 7.0, rect.bottom() - 7.0),
            Align2::LEFT_CENTER,
            self.playback_status(),
            FontId::monospace(7.0),
            skin::LCD_DIM,
        );
        painter.text(
            pos2(rect.left() + 92.0, rect.top() + 13.0),
            Align2::LEFT_CENTER,
            title,
            FontId::monospace(9.0),
            skin::LCD_GREEN,
        );
        painter.text(
            pos2(rect.left() + 92.0, rect.top() + 27.0),
            Align2::LEFT_CENTER,
            artist,
            FontId::monospace(7.5),
            skin::LCD_DIM,
        );
        painter.text(
            pos2(rect.right() - 6.0, rect.bottom() - 7.0),
            Align2::RIGHT_CENTER,
            "SPECTRUM // N/A",
            FontId::monospace(6.5),
            skin::LCD_DIM,
        );
    }

    fn playback_status(&self) -> &'static str {
        match self.snapshot.state {
            PlaybackState::Stopped => "STOPPED",
            PlaybackState::Loading => "OPENING",
            PlaybackState::Playing => "PLAYING",
            PlaybackState::Paused => "PAUSED",
        }
    }

    fn show_seek(&mut self, ui: &mut egui::Ui) {
        let max_seek = self
            .snapshot
            .duration
            .map(|duration| duration.as_secs_f32())
            .unwrap_or(1.0)
            .max(1.0);
        if !self.seek_dragging {
            if let Some(position) = self.snapshot.position {
                self.seek_value = position.as_secs_f64();
            }
        }
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format_time(self.snapshot.position))
                    .monospace()
                    .size(8.0)
                    .color(skin::LCD_DIM),
            );
            let slider_width = (ui.available_width() - 38.0).max(72.0);
            let mut position = self.seek_value as f32;
            let response = skin::h_slider(ui, &mut position, 0.0..=max_seek, slider_width);
            self.seek_value = f64::from(position);
            self.seek_dragging = response.dragged() || response.is_pointer_button_down_on();
            if (response.drag_stopped() || response.clicked()) && self.snapshot.duration.is_some() {
                self.playback
                    .seek(Duration::from_secs_f32(position.clamp(0.0, max_seek)));
            }
            ui.label(
                RichText::new(format_time(self.snapshot.duration))
                    .monospace()
                    .size(8.0)
                    .color(skin::LCD_DIM),
            );
        });
    }

    fn show_transport(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if skin::transport_button(ui, skin::TransportIcon::Previous, false).clicked() {
                self.play_next(-1);
            }
            if skin::transport_button(
                ui,
                skin::TransportIcon::Play,
                self.snapshot.state == PlaybackState::Playing,
            )
            .clicked()
            {
                self.play_selected();
            }
            if skin::transport_button(
                ui,
                skin::TransportIcon::Pause,
                self.snapshot.state == PlaybackState::Paused,
            )
            .clicked()
            {
                match pause_action(&self.snapshot) {
                    PauseAction::Play => self.play_selected(),
                    PauseAction::Pause(Some(request_id)) => {
                        self.playback.pause_for_request(request_id);
                    }
                    PauseAction::Pause(None) => self.playback.pause(),
                }
            }
            if skin::transport_button(ui, skin::TransportIcon::Stop, false).clicked() {
                self.stop_playback();
            }
            if skin::transport_button(ui, skin::TransportIcon::Next, false).clicked() {
                self.play_next(1);
            }
            ui.add_space(4.0);
            ui.label(
                RichText::new("VOL")
                    .monospace()
                    .size(7.0)
                    .color(skin::TEXT_DIM),
            );
            let slider_width = (ui.available_width() - 48.0).max(48.0);
            let mut volume = f32::from(self.volume);
            let response = skin::h_slider(ui, &mut volume, 0.0..=100.0, slider_width);
            if response.dragged() || response.drag_stopped() || response.clicked() {
                let new_volume = volume.round() as u8;
                if new_volume != self.volume {
                    self.volume = new_volume;
                    self.playback.set_volume(new_volume);
                }
            }
            if skin::text_button(ui, "M", vec2(20.0, 18.0), self.muted, true).clicked() {
                self.muted = !self.muted;
                self.playback.set_muted(self.muted);
            }
        });
    }

    fn show_playback_options(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("SPD")
                    .monospace()
                    .size(7.0)
                    .color(skin::TEXT_DIM),
            );
            egui::ComboBox::from_id_salt("playback-speed")
                .selected_text(format!("{:.2}x", self.rate))
                .width(55.0)
                .show_ui(ui, |ui| {
                    for rate in [0.5_f32, 0.75, 1.0, 1.25, 1.5, 2.0] {
                        if ui
                            .selectable_value(&mut self.rate, rate, format!("{rate:.2}x"))
                            .changed()
                        {
                            self.playback.set_rate(rate);
                        }
                    }
                });
            self.show_track_menus(ui);
            if self
                .playlist
                .current
                .and_then(|index| self.playlist.tracks.get(index))
                .is_some_and(|track| track.media_type == MediaType::Video)
                && skin::text_button(ui, "VID", vec2(28.0, 18.0), self.video_visible, true)
                    .clicked()
            {
                self.toggle_video(ui.ctx());
            }
        });
    }

    fn show_track_menus(&mut self, ui: &mut egui::Ui) {
        if !self.snapshot.audio_tracks.is_empty() {
            egui::ComboBox::from_id_salt("audio-track")
                .selected_text("AUD")
                .width(42.0)
                .show_ui(ui, |ui| {
                    let tracks = self.snapshot.audio_tracks.clone();
                    for track in tracks {
                        if ui.selectable_label(false, &track.label).clicked() {
                            self.playback.set_audio_track(track.id);
                        }
                    }
                });
        }
        if !self.snapshot.subtitles.is_empty() {
            egui::ComboBox::from_id_salt("subtitle-track")
                .selected_text("SUB")
                .width(42.0)
                .show_ui(ui, |ui| {
                    let tracks = self.snapshot.subtitles.clone();
                    for track in tracks {
                        if ui.selectable_label(false, &track.label).clicked() {
                            self.playback.set_subtitle(track.id);
                        }
                    }
                    if ui.selectable_label(false, "Off").clicked() {
                        self.playback.set_subtitle(-1);
                    }
                });
        }
        if skin::text_button(ui, "SRT+", vec2(30.0, 18.0), false, true).clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("Subtitles", &["srt", "ass"])
                .pick_file()
            {
                self.playback.add_subtitle(path);
            }
        }
    }

    fn toggle_video(&mut self, context: &egui::Context) {
        if !self.video_open {
            return;
        }
        self.video_visible = !self.video_visible;
        context.send_viewport_cmd_to(
            video_viewport(),
            egui::ViewportCommand::Visible(self.video_visible),
        );
    }

    fn show_playlist(&mut self, ui: &mut egui::Ui) {
        let title = skin::title_bar(ui, "PLAYLIST EDITOR", &[("X", false)]);
        if title.drag_started {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if title.clicked == Some(0) {
            self.playlist_open = false;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.horizontal(|ui| {
            if skin::text_button(ui, "ADD FILES", vec2(56.0, 18.0), false, true).clicked() {
                self.choose_files();
            }
            if skin::text_button(ui, "REMOVE", vec2(48.0, 18.0), false, true).clicked() {
                let removed = self.playlist.remove_selected();
                if removed
                    .as_ref()
                    .is_some_and(|path| self.snapshot.path.as_ref() == Some(path))
                {
                    self.stop_playback();
                    self.close_video();
                }
            }
            if skin::text_button(ui, "CLEAR", vec2(42.0, 18.0), false, true).clicked() {
                self.playlist.clear();
                self.stop_playback();
                self.close_video();
            }
        });
        ui.add_space(4.0);
        Frame::new()
            .fill(skin::PANEL_DARK)
            .stroke(Stroke::new(1.0, skin::BEVEL_LIGHT))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                egui::Grid::new("playlist-header")
                    .num_columns(3)
                    .striped(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new("TITLE").color(skin::ACCENT));
                        ui.label(RichText::new("TYPE").color(skin::ACCENT));
                        ui.label(RichText::new("LENGTH").color(skin::ACCENT));
                        ui.end_row();
                    });
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for index in 0..self.playlist.tracks.len() {
                            let track = &self.playlist.tracks[index];
                            let title = track.title.clone();
                            let artist = track.artist.clone();
                            let duration = track.duration;
                            let media_type = track.media_type;
                            let text = if let Some(artist) = artist.as_deref() {
                                format!("{title}  —  {artist}")
                            } else {
                                title
                            };
                            let selected = self.playlist.selected == Some(index);
                            let is_current = self.playlist.current == Some(index);
                            let response = ui.selectable_label(
                                selected,
                                RichText::new(text).color(if is_current {
                                    skin::LCD_GREEN
                                } else {
                                    skin::TITLE_TEXT
                                }),
                            );
                            if response.clicked() {
                                self.playlist.selected = Some(index);
                            }
                            if response.double_clicked() {
                                self.play_index(index);
                            }
                            ui.label(match media_type {
                                MediaType::Audio => "AUDIO",
                                MediaType::Video => "VIDEO",
                            });
                            ui.label(format_time(duration));
                            ui.end_row();
                        }
                    });
            });
        ui.add_space(5.0);
        ui.label(RichText::new(self.playlist.summary()).color(skin::LCD_GREEN));
        ui.label(
            RichText::new("Drop MP3 / FLAC / WAV / MP4 / MKV / AVI files here.")
                .color(skin::TEXT_DIM)
                .small(),
        );
        let dropped = ui.ctx().input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped);
        }
    }

    fn show_equalizer(&mut self, ui: &mut egui::Ui) {
        let title = skin::title_bar(ui, "EQUALIZER", &[("X", false)]);
        if title.drag_started {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if title.clicked == Some(0) {
            self.equalizer_open = false;
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.snapshot.equalizer_supported {
            ui.label(RichText::new("DSP: LIBVLC EQUALIZER").color(skin::LCD_GREEN));
        } else {
            ui.label(
                RichText::new("DSP NOT AVAILABLE IN THE LOADED VLC RUNTIME")
                    .color(Color32::from_rgb(255, 160, 90)),
            );
        }
        let mut changed = false;
        ui.add_enabled_ui(self.snapshot.equalizer_supported, |ui| {
            ui.horizontal(|ui| {
                if skin::text_button(
                    ui,
                    "EQ ON",
                    vec2(40.0, 18.0),
                    self.equalizer_enabled,
                    self.snapshot.equalizer_supported,
                )
                .clicked()
                {
                    self.equalizer_enabled = !self.equalizer_enabled;
                    changed = true;
                }
                ui.label("PREAMP");
                let slider_width = (ui.available_width() - 70.0).max(80.0);
                let previous_preamp = self.preamp;
                skin::h_slider(ui, &mut self.preamp, -20.0..=20.0, slider_width);
                changed |= self.preamp != previous_preamp;
                ui.label(
                    RichText::new(format!("{:.1}", self.preamp))
                        .monospace()
                        .size(8.0),
                );
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                for (index, band) in self.bands.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        let previous_band = *band;
                        skin::v_slider(
                            ui,
                            band,
                            -20.0..=20.0,
                            vec2(20.0, 88.0),
                            self.snapshot.equalizer_supported,
                        );
                        changed |= *band != previous_band;
                        ui.label(RichText::new(BANDS[index]).color(skin::TEXT_DIM).size(7.0));
                    });
                }
            });
        });
        ui.horizontal(|ui| {
            if skin::text_button(
                ui,
                "FLAT / RESET",
                vec2(68.0, 18.0),
                false,
                self.snapshot.equalizer_supported,
            )
            .clicked()
            {
                self.preamp = 0.0;
                self.bands = [0.0; 10];
                changed = true;
            }
        });
        if changed {
            self.playback
                .set_equalizer(self.equalizer_enabled, self.preamp, self.bands);
        }
    }

    fn show_video(&mut self, ui: &mut egui::Ui, root_call_seq: u64, diagnostic_sample: bool) {
        let context = ui.ctx().clone();
        if context.input(|input| input.key_pressed(egui::Key::Escape)) && self.video_fullscreen {
            self.video_fullscreen = false;
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested {
            self.hide_video(&context);
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }

        let viewport_rect = ui.max_rect();
        let cursor_at_entry = ui.cursor().min;
        ui.painter().rect_filled(viewport_rect, 0.0, Color32::BLACK);
        let mut close_video = false;
        let mut title_bar_rect = None;
        let mut close_button_rect = None;
        let mut fullscreen_button_rect = None;
        let mut fullscreen_help_rect = None;
        if !self.video_fullscreen {
            let title = skin::title_bar(ui, "RETROWAVE VIDEO", &[("X", false)]);
            title_bar_rect = Some(title.rect);
            close_button_rect = title.first_button_rect;
            let pointer_pressed =
                context.input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
            if pointer_pressed && title.drag_hovered {
                let pointer = context.input(|input| input.pointer.interact_pos());
                log_egui_record(
                    &context,
                    "video",
                    root_call_seq,
                    "input-title-hit",
                    format_args!(
                        "zone={:?} pointer={pointer:?} pointer_hit=true",
                        title.drag_rect
                    ),
                );
            }
            if title.drag_started {
                context.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                log_egui_record(
                    &context,
                    "video",
                    root_call_seq,
                    "input-title-drag-start",
                    format_args!(
                        "zone={:?} pointer={:?} pointer_hit=true command=StartDrag-sent",
                        title.drag_rect, title.drag_pointer_pos
                    ),
                );
                self.video_title_drag_continuation_logged = false;
            } else if title.dragged && !self.video_title_drag_continuation_logged {
                log_egui_record(
                    &context,
                    "video",
                    root_call_seq,
                    "input-title-drag-continue",
                    format_args!(
                        "zone={:?} pointer={:?} pointer_hit=true",
                        title.drag_rect, title.drag_pointer_pos
                    ),
                );
                self.video_title_drag_continuation_logged = true;
            }
            if title.drag_stopped {
                log_egui_record(
                    &context,
                    "video",
                    root_call_seq,
                    "input-title-drag-stop",
                    format_args!(
                        "zone={:?} pointer={:?}",
                        title.drag_rect, title.drag_pointer_pos
                    ),
                );
                self.video_title_drag_continuation_logged = false;
            }
            if title.clicked == Some(0) {
                let pointer = context.input(|input| input.pointer.interact_pos());
                log_egui_record(
                    &context,
                    "video",
                    root_call_seq,
                    "input-title-close-click",
                    format_args!(
                        "zone={:?} pointer={pointer:?} pointer_hit=true",
                        title.first_button_rect
                    ),
                );
            }
            close_video = title.clicked == Some(0);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    let fullscreen_button =
                        skin::text_button(ui, "FULLSCREEN", vec2(64.0, 18.0), false, true);
                    fullscreen_button_rect = Some(fullscreen_button.rect);
                    if fullscreen_button.clicked() {
                        let pointer = fullscreen_button.interact_pointer_pos();
                        log_egui_record(
                            &context,
                            "video",
                            root_call_seq,
                            "input-fullscreen-click",
                            format_args!(
                                "zone={:?} pointer={pointer:?} pointer_hit=true",
                                fullscreen_button.rect
                            ),
                        );
                        self.video_fullscreen = true;
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    }
                    let fullscreen_help = ui.label(
                        RichText::new("VIDEO // ESC EXITS FULLSCREEN")
                            .color(skin::TEXT_DIM)
                            .monospace()
                            .size(8.0),
                    );
                    fullscreen_help_rect = Some(fullscreen_help.rect);
                });
            });
        }
        if close_video {
            self.hide_video(&context);
            return;
        }

        let scale = context.pixels_per_point();
        let video_rect = video_output_rect(viewport_rect, self.video_fullscreen);
        if !self.video_fullscreen {
            self.add_video_resize_handles(ui, viewport_rect, root_call_seq);
        }
        self.log_video_ui_geometry(
            ui,
            cursor_at_entry,
            title_bar_rect,
            close_button_rect,
            fullscreen_button_rect,
            fullscreen_help_rect,
            root_call_seq,
            diagnostic_sample,
        );
        let parent = find_video_window();
        if parent == 0 {
            if self.pending_video_start.take().is_some() {
                self.notice =
                    Some("Could not locate the RetroWave Video window for VLC.".to_owned());
                self.video_request_id = None;
                self.video_open = false;
                self.video_visible = false;
            }
            return;
        }
        if self.video_output_hwnd == 0 {
            match create_video_output_window(parent) {
                Ok(hwnd) => self.video_output_hwnd = hwnd,
                Err(error) => {
                    self.notice = Some(error);
                    self.pending_video_start = None;
                    self.video_request_id = None;
                    self.video_open = false;
                    self.video_visible = false;
                    return;
                }
            }
        }
        if let Err(error) = position_video_output_window(
            parent,
            self.video_output_hwnd,
            viewport_rect,
            video_rect,
            scale,
            self.video_open,
            self.video_visible,
            &mut self.last_video_window_diagnostic,
            &mut self.last_video_window_diagnostic_at,
            root_call_seq,
            diagnostic_sample,
        ) {
            self.notice = Some(error);
            return;
        }
        set_video_output_visible(
            self.video_output_hwnd,
            self.video_open && self.video_visible,
        );
        if let Some((path, request_id)) = self.pending_video_start.take() {
            self.playback
                .play(path, self.video_output_hwnd, request_id, true);
        }
    }

    fn log_video_ui_geometry(
        &self,
        ui: &egui::Ui,
        cursor_at_entry: egui::Pos2,
        title_bar: Option<egui::Rect>,
        close_button: Option<egui::Rect>,
        fullscreen_button: Option<egui::Rect>,
        fullscreen_help: Option<egui::Rect>,
        root_call_seq: u64,
        diagnostic_sample: bool,
    ) {
        if !diagnostic_sample {
            return;
        }
        let context = ui.ctx();
        let viewport = context.input(|input| input.viewport().clone());
        log_egui_record(
            context,
            "video",
            root_call_seq,
            "ui-layout",
            format_args!(
                "phase_sample=true viewport_info={viewport:?} viewport_rect={:?} \
             ui_max_rect_points={:?} ui_available_rect_points={:?} ui_min_rect_points={:?} \
             ui_clip_rect_points={:?} cursor_at_entry_points={cursor_at_entry:?} \
             cursor_at_end_points={:?} title_bar_points={title_bar:?} \
             close_button_points={close_button:?} fullscreen_button_points={fullscreen_button:?} \
             fullscreen_help_points={fullscreen_help:?} pixels_per_point={:.3} \
             video_open={} video_visible={} video_fullscreen={} child_hwnd=0x{:X}",
                context.viewport_rect(),
                ui.max_rect(),
                ui.available_rect_before_wrap(),
                ui.min_rect(),
                ui.clip_rect(),
                ui.cursor().min,
                context.pixels_per_point(),
                self.video_open,
                self.video_visible,
                self.video_fullscreen,
                self.video_output_hwnd,
            ),
        );
    }

    fn add_video_resize_handles(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        root_call_seq: u64,
    ) {
        let edge = 6.0;
        let corner = 14.0;
        let middle_left = rect.left() + corner;
        let middle_right = rect.right() - corner;
        let side_top = rect.top() + 18.0 + edge;
        let side_bottom = rect.bottom() - 24.0 - edge;
        let handles = [
            (
                egui::viewport::ResizeDirection::NorthWest,
                egui::Rect::from_min_max(rect.min, rect.min + egui::vec2(corner, edge)),
            ),
            (
                egui::viewport::ResizeDirection::North,
                egui::Rect::from_min_max(
                    egui::pos2(middle_left, rect.top()),
                    egui::pos2(middle_right, rect.top() + edge),
                ),
            ),
            (
                egui::viewport::ResizeDirection::NorthEast,
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - corner, rect.top()),
                    egui::pos2(rect.right(), rect.top() + edge),
                ),
            ),
            (
                egui::viewport::ResizeDirection::West,
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), side_top),
                    egui::pos2(rect.left() + edge, side_bottom),
                ),
            ),
            (
                egui::viewport::ResizeDirection::East,
                egui::Rect::from_min_max(
                    egui::pos2(rect.right() - edge, side_top),
                    egui::pos2(rect.right(), side_bottom),
                ),
            ),
            (
                egui::viewport::ResizeDirection::SouthWest,
                egui::Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - edge),
                    egui::pos2(rect.left() + corner, rect.bottom()),
                ),
            ),
            (
                egui::viewport::ResizeDirection::South,
                egui::Rect::from_min_max(
                    egui::pos2(middle_left, rect.bottom() - edge),
                    egui::pos2(middle_right, rect.bottom()),
                ),
            ),
            (
                egui::viewport::ResizeDirection::SouthEast,
                egui::Rect::from_min_max(rect.max - egui::vec2(corner, edge), rect.max),
            ),
        ];
        let pointer_pressed = ui
            .ctx()
            .input(|input| input.pointer.button_pressed(egui::PointerButton::Primary));
        let mut pointer_hit_resize_zone = false;
        for (index, (direction, handle_rect)) in handles.into_iter().enumerate() {
            let response = ui.interact(
                handle_rect,
                ui.scope_id().with(("video-window-resize", index)),
                egui::Sense::click_and_drag(),
            );
            if pointer_pressed && response.hovered() {
                pointer_hit_resize_zone = true;
                let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
                log_egui_record(
                    ui.ctx(),
                    "video",
                    root_call_seq,
                    "input-resize-hit",
                    format_args!(
                        "direction={direction:?} zone={handle_rect:?} \
                         pointer={pointer:?} pointer_hit=true"
                    ),
                );
            }
            if response.drag_started() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                self.video_resize_drag_index = Some(index);
                self.video_resize_drag_continuation_logged = false;
                let pointer = response.interact_pointer_pos();
                log_egui_record(
                    ui.ctx(),
                    "video",
                    root_call_seq,
                    "input-resize-start",
                    format_args!(
                        "direction={direction:?} zone={handle_rect:?} pointer={pointer:?} \
                         pointer_hit={} command=BeginResize-sent",
                        pointer.is_some_and(|position| handle_rect.contains(position))
                    ),
                );
            } else if response.dragged()
                && self.video_resize_drag_index == Some(index)
                && !self.video_resize_drag_continuation_logged
            {
                self.video_resize_drag_continuation_logged = true;
                log_egui_record(
                    ui.ctx(),
                    "video",
                    root_call_seq,
                    "input-resize-continue",
                    format_args!(
                        "direction={direction:?} zone={handle_rect:?} \
                         pointer={:?}",
                        response.interact_pointer_pos()
                    ),
                );
            }
            if response.drag_stopped() && self.video_resize_drag_index == Some(index) {
                log_egui_record(
                    ui.ctx(),
                    "video",
                    root_call_seq,
                    "input-resize-stop",
                    format_args!(
                        "direction={direction:?} zone={handle_rect:?} \
                         pointer={:?}",
                        response.interact_pointer_pos()
                    ),
                );
                self.video_resize_drag_index = None;
                self.video_resize_drag_continuation_logged = false;
            }
        }
        if pointer_pressed && !pointer_hit_resize_zone {
            let pointer = ui.ctx().input(|input| input.pointer.interact_pos());
            log_egui_record(
                ui.ctx(),
                "video",
                root_call_seq,
                "input-resize-no-hit",
                format_args!("pointer={pointer:?} pointer_hit=false"),
            );
        }
    }

    fn render_child_windows(&mut self, context: &egui::Context, root_call_seq: u64) {
        if self.playlist_open {
            let close_requested = std::cell::Cell::new(false);
            context.show_viewport_immediate(
                playlist_viewport(),
                ViewportBuilder::default()
                    .with_title("RetroWave Playlist")
                    .with_inner_size([500.0, 330.0])
                    .with_min_inner_size([380.0, 220.0])
                    .with_decorations(false),
                |ui, _| {
                    skin::apply(ui.ctx());
                    close_requested.set(ui.ctx().input(|input| input.viewport().close_requested()));
                    egui::CentralPanel::default().show(ui, |ui| self.show_playlist(ui));
                },
            );
            if close_requested.get() {
                self.playlist_open = false;
            }
        }
        if self.equalizer_open {
            let close_requested = std::cell::Cell::new(false);
            context.show_viewport_immediate(
                equalizer_viewport(),
                ViewportBuilder::default()
                    .with_title("RetroWave Equalizer")
                    .with_inner_size([300.0, 195.0])
                    .with_min_inner_size([300.0, 195.0])
                    .with_max_inner_size([300.0, 195.0])
                    .with_resizable(false)
                    .with_decorations(false),
                |ui, _| {
                    skin::apply(ui.ctx());
                    close_requested.set(ui.ctx().input(|input| input.viewport().close_requested()));
                    egui::CentralPanel::default().show(ui, |ui| self.show_equalizer(ui));
                },
            );
            if close_requested.get() {
                self.equalizer_open = false;
            }
        }
        if self.video_container_created {
            let close_requested = std::cell::Cell::new(false);
            context.show_viewport_immediate(
                video_viewport(),
                ViewportBuilder::default()
                    .with_title(VIDEO_TITLE)
                    .with_inner_size([720.0, 405.0])
                    .with_min_inner_size([320.0, 180.0])
                    .with_resizable(true)
                    .with_decorations(false)
                    .with_visible(self.video_visible)
                    .with_fullscreen(self.video_fullscreen),
                |ui, _| {
                    let video_context = ui.ctx().clone();
                    let viewport_info = video_context.input(|input| input.viewport().clone());
                    let (diagnostic_sample, phase) = update_diagnostic_sample(
                        ViewportDiagnosticState::from(&viewport_info),
                        &mut self.last_video_diagnostic_state,
                        &mut self.video_restore_diagnostic_frames_remaining,
                    );
                    if diagnostic_sample {
                        log_egui_record(
                            &video_context,
                            "video",
                            root_call_seq,
                            "ui-callback-enter",
                            format_args!(
                                "phase={phase} viewport_info={viewport_info:?} \
                                 viewport_rect={:?} pixels_per_point={:.3} \
                                 native_hwnd=0x{:X}",
                                video_context.viewport_rect(),
                                video_context.pixels_per_point(),
                                self.video_output_hwnd,
                            ),
                        );
                    }
                    close_requested.set(ui.ctx().input(|input| input.viewport().close_requested()));
                    if self.video_open {
                        self.show_video(ui, root_call_seq, diagnostic_sample);
                    } else {
                        ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
                    }
                    if diagnostic_sample {
                        log_egui_record(
                            &video_context,
                            "video",
                            root_call_seq,
                            "ui-callback-exit",
                            format_args!("phase={phase}"),
                        );
                    }
                },
            );
            if close_requested.get() {
                self.hide_video(context);
                context.send_viewport_cmd_to(video_viewport(), egui::ViewportCommand::CancelClose);
            }
        }
    }
}

impl eframe::App for RetroWaveApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root_ui.ctx().clone();
        self.diagnostic_root_call_seq += 1;
        let root_call_seq = self.diagnostic_root_call_seq;
        let (root_viewport_info, video_viewport_info) = context.input(|input| {
            (
                input.viewport().clone(),
                input.raw.viewports.get(&video_viewport()).cloned(),
            )
        });
        let (root_diagnostic_sample, root_phase) = update_diagnostic_sample(
            ViewportDiagnosticState::from(&root_viewport_info),
            &mut self.last_root_diagnostic_state,
            &mut self.root_restore_diagnostic_frames_remaining,
        );
        let (root_video_diagnostic_sample, root_video_phase) =
            if let Some(video_info) = &video_viewport_info {
                update_diagnostic_sample(
                    ViewportDiagnosticState::from(video_info),
                    &mut self.last_root_video_diagnostic_state,
                    &mut self.root_video_restore_diagnostic_frames_remaining,
                )
            } else {
                (false, "unavailable")
            };
        let root_diagnostic_sample = root_diagnostic_sample || root_video_diagnostic_sample;
        if root_diagnostic_sample {
            log_egui_record(
                &context,
                "root",
                root_call_seq,
                "ui-callback-enter",
                format_args!(
                    "root_phase={root_phase} video_phase={root_video_phase} \
                     root_viewport_info={root_viewport_info:?} \
                     video_viewport_info={video_viewport_info:?} \
                     native_root_hwnd=unavailable(reason=eframe_App_api_does_not_expose_handle)"
                ),
            );
        }
        self.snapshot = self.playback.snapshot();
        self.update_metadata();
        self.consume_natural_end();
        skin::apply(&context);

        let dropped = context.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if !dropped.is_empty() {
            self.add_paths(dropped);
        }

        Frame::new()
            .fill(skin::PANEL_DARK)
            .inner_margin(Margin::same(3))
            .show(root_ui, |ui| {
                ui.set_min_height(ui.available_height());
                self.show_player(ui);
            });
        if root_diagnostic_sample {
            log_egui_record(
                &context,
                "root",
                root_call_seq,
                "ui-layout",
                format_args!(
                    "root_viewport_rect={:?} ui_max_rect_points={:?} \
                     ui_available_rect_points={:?} ui_min_rect_points={:?} \
                     ui_clip_rect_points={:?} cursor_points={:?} \
                     pixels_per_point={:.3}",
                    context.viewport_rect(),
                    root_ui.max_rect(),
                    root_ui.available_rect_before_wrap(),
                    root_ui.min_rect(),
                    root_ui.clip_rect(),
                    context.input(|input| input.pointer.interact_pos()),
                    context.pixels_per_point(),
                ),
            );
        }
        self.render_child_windows(&context, root_call_seq);
        if root_diagnostic_sample {
            log_egui_record(
                &context,
                "root",
                root_call_seq,
                "ui-callback-exit",
                format_args!("phase={root_phase} video_phase={root_video_phase}"),
            );
        }
        context.request_repaint_after(Duration::from_millis(100));
    }
}

fn apply_metadata(track: &mut Track, metadata: &TrackMetadata) {
    if let Some(title) = &metadata.title {
        track.title.clone_from(title);
    }
    track.artist.clone_from(&metadata.artist);
    track.album.clone_from(&metadata.album);
    track.duration = metadata.duration;
}

fn playlist_current_for_path(
    tracks: &[Track],
    current: Option<usize>,
    path: Option<&PathBuf>,
) -> Option<usize> {
    let path = path?;
    current
        .filter(|&index| tracks.get(index).is_some_and(|track| &track.path == path))
        .or_else(|| tracks.iter().position(|track| &track.path == path))
}

fn confirmed_video_path(active_request_id: u64, result: &VideoPlayResult) -> Option<PathBuf> {
    (result.request_id == active_request_id).then(|| result.path.clone())
}

fn reset_video_viewport_state(
    video_open: &mut bool,
    video_visible: &mut bool,
    video_fullscreen: &mut bool,
    pending_video_start: &mut Option<(PathBuf, u64)>,
    video_request_id: &mut Option<u64>,
) {
    *video_open = false;
    *video_visible = false;
    *video_fullscreen = false;
    *pending_video_start = None;
    *video_request_id = None;
}

fn video_output_rect(viewport: egui::Rect, fullscreen: bool) -> egui::Rect {
    if fullscreen {
        viewport
    } else {
        egui::Rect::from_min_max(
            egui::pos2(viewport.left() + 6.0, viewport.top() + 18.0),
            egui::pos2(viewport.right() - 6.0, viewport.bottom() - 24.0),
        )
    }
}

#[cfg(windows)]
fn create_video_output_window(parent: usize) -> Result<usize, String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, WINDOW_EX_STYLE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
    };
    use windows::core::w;

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("STATIC"),
            w!(""),
            WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS | WS_VISIBLE,
            0,
            0,
            1,
            1,
            Some(HWND(parent as *mut std::ffi::c_void)),
            None,
            None,
            None,
        )
    }
    .map_err(|error| format!("Could not create the VLC video output window: {error}"))?;
    Ok(hwnd.0 as usize)
}

#[cfg(not(windows))]
fn create_video_output_window(_parent: usize) -> Result<usize, String> {
    Err("The VLC video output window is supported only on Windows.".to_owned())
}

#[cfg(windows)]
fn position_video_output_window(
    parent: usize,
    child: usize,
    viewport: egui::Rect,
    video: egui::Rect,
    scale: f32,
    video_open: bool,
    video_visible: bool,
    last_state: &mut Option<VideoWindowDiagnosticState>,
    last_diagnostic_at: &mut Option<Instant>,
    root_call_seq: u64,
    diagnostic_sample: bool,
) -> Result<(), String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsWindowVisible, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    };

    let x = ((video.left() - viewport.left()) * scale).round() as i32;
    let y = ((video.top() - viewport.top()) * scale).round() as i32;
    let width = (video.width() * scale).round().max(1.0) as i32;
    let height = (video.height() * scale).round().max(1.0) as i32;
    let parent_hwnd = HWND(parent as *mut std::ffi::c_void);
    let child_hwnd = HWND(child as *mut std::ffi::c_void);
    let parent_minimized = unsafe { IsIconic(parent_hwnd) }.as_bool();
    let native_state_sample = diagnostic_sample
        || last_state.is_none_or(|previous_state| {
            previous_state.parent_hwnd != parent
                || previous_state.child_hwnd != child
                || previous_state.parent_minimized != parent_minimized
                || previous_state.video_open != video_open
                || previous_state.video_visible != video_visible
        });
    let (parent_visible, child_visible) = if native_state_sample {
        (
            unsafe { IsWindowVisible(parent_hwnd) }.as_bool(),
            unsafe { IsWindowVisible(child_hwnd) }.as_bool(),
        )
    } else {
        last_state
            .map(|previous_state| (previous_state.parent_visible, previous_state.child_visible))
            .unwrap_or((false, false))
    };
    let state = VideoWindowDiagnosticState {
        parent_hwnd: parent,
        child_hwnd: child,
        viewport_rect: viewport,
        video_rect: video,
        pixels_per_point: scale,
        set_window_pos: (x, y, width, height),
        parent_minimized,
        parent_visible,
        child_visible,
        video_open,
        video_visible,
    };
    let now = Instant::now();
    let unchanged = *last_state == Some(state);
    let lifecycle_unchanged =
        last_state.is_some_and(|previous_state| previous_state.same_lifecycle_state(&state));
    if parent_minimized {
        let lifecycle_changed = !lifecycle_unchanged;
        if lifecycle_changed || diagnostic_sample {
            let (parent_thread, parent_pid, child_thread, child_pid) =
                log_video_window_process_identity(parent_hwnd, child_hwnd, root_call_seq);
            log_diagnostic_record(
                "video-window",
                root_call_seq,
                format_args!(
                    "event=skip-position-minimized reason={} parent_hwnd=0x{parent:X} \
                     parent_pid={parent_pid} parent_thread={parent_thread} \
                     parent_is_current_process={} \
                     child_hwnd=0x{child:X} child_pid={child_pid} child_thread={child_thread} \
                     current_pid={} parent_visible={parent_visible} child_visible={child_visible} \
                     video_open={video_open} video_visible={video_visible} \
                     viewport_points={viewport:?} video_points={video:?} \
                     pixels_per_point={scale:.3}",
                    if lifecycle_changed {
                        "minimized-transition"
                    } else {
                        "sample"
                    },
                    parent_pid == std::process::id(),
                    std::process::id()
                ),
            );
            if lifecycle_changed {
                log_video_window_identity(parent_hwnd, root_call_seq);
            }
            log_video_window_geometry(parent_hwnd, child_hwnd, state.set_window_pos, root_call_seq);
        }
        *last_state = Some(state);
        *last_diagnostic_at = Some(now);
        return Ok(());
    }
    let heartbeat_due =
        last_diagnostic_at.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1));
    if unchanged && !heartbeat_due && !diagnostic_sample {
        return Ok(());
    }

    let log_state = !lifecycle_unchanged || diagnostic_sample;
    if log_state {
        let (parent_thread, parent_pid, child_thread, child_pid) =
            log_video_window_process_identity(parent_hwnd, child_hwnd, root_call_seq);
        log_diagnostic_record(
            "video-window",
            root_call_seq,
            format_args!(
                "event=window-state reason={} parent_hwnd=0x{parent:X} \
                 parent_pid={parent_pid} parent_thread={parent_thread} child_hwnd=0x{child:X} \
                 parent_is_current_process={} child_pid={child_pid} child_thread={child_thread} \
                 current_pid={} \
                 parent_minimized={parent_minimized} parent_visible={parent_visible} \
                 child_visible={child_visible} video_open={video_open} \
                 video_visible={video_visible} viewport_points={viewport:?} \
                 video_points={video:?} pixels_per_point={scale:.3} \
                 set_window_pos=(x:{x}, y:{y}, width:{width}, height:{height})",
                if lifecycle_unchanged {
                    "sample"
                } else {
                    "state-change"
                },
                parent_pid == std::process::id(),
                std::process::id()
            ),
        );
        if !lifecycle_unchanged {
            log_video_window_identity(parent_hwnd, root_call_seq);
        }
    }

    let position_result = if !unchanged || heartbeat_due {
        Some(unsafe {
            SetWindowPos(
                child_hwnd,
                None,
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        })
    } else {
        None
    };
    if let Some(result) = &position_result {
        match result {
            Ok(()) if log_state => log_diagnostic_record(
                "video-window",
                root_call_seq,
                "event=set-window-pos result=ok",
            ),
            Ok(()) => {}
            Err(error) => log_diagnostic_record(
                "video-window",
                root_call_seq,
                format_args!("event=error set-window-pos result=error detail={error}"),
            ),
        }
    }
    let result = match position_result {
        Some(result) => {
            result.map_err(|error| format!("Could not resize the VLC video output window: {error}"))
        }
        None => Ok(()),
    };
    if log_state || result.is_err() {
        log_video_window_geometry(parent_hwnd, child_hwnd, state.set_window_pos, root_call_seq);
    }
    if !unchanged || heartbeat_due {
        *last_state = Some(state);
        *last_diagnostic_at = Some(now);
    }
    if result.is_err() {
        log_diagnostic_record(
            "video-window",
            root_call_seq,
            "event=retry set-window-pos will retry on next change or heartbeat",
        );
    }
    result
}

#[cfg(windows)]
fn log_video_window_process_identity(
    parent: windows::Win32::Foundation::HWND,
    child: windows::Win32::Foundation::HWND,
    root_call_seq: u64,
) -> (u32, u32, u32, u32) {
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut parent_pid = 0u32;
    let mut child_pid = 0u32;
    let parent_thread =
        unsafe { GetWindowThreadProcessId(parent, Some(&mut parent_pid as *mut u32)) };
    let child_thread = unsafe { GetWindowThreadProcessId(child, Some(&mut child_pid as *mut u32)) };
    if parent_thread == 0 || parent_pid == 0 {
        log_diagnostic_record(
            "video-window",
            root_call_seq,
            "event=error GetWindowThreadProcessId failed for parent HWND",
        );
    }
    if child_thread == 0 || child_pid == 0 {
        log_diagnostic_record(
            "video-window",
            root_call_seq,
            "event=error GetWindowThreadProcessId failed for child HWND",
        );
    }
    (parent_thread, parent_pid, child_thread, child_pid)
}

#[cfg(not(windows))]
fn position_video_output_window(
    _parent: usize,
    _child: usize,
    _viewport: egui::Rect,
    _video: egui::Rect,
    _scale: f32,
    _video_open: bool,
    _video_visible: bool,
    _last_state: &mut Option<VideoWindowDiagnosticState>,
    _last_diagnostic_at: &mut Option<Instant>,
    _root_call_seq: u64,
    _diagnostic_sample: bool,
) -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn log_video_window_identity(parent: windows::Win32::Foundation::HWND, root_call_seq: u64) {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowTextW};

    let mut title_buffer = [0u16; 256];
    let title_len = unsafe { GetWindowTextW(parent, &mut title_buffer) };
    let mut class_buffer = [0u16; 256];
    let class_len = unsafe { GetClassNameW(parent, &mut class_buffer) };
    if title_len == 0 {
        log_diagnostic_record(
            "video-window",
            root_call_seq,
            "event=error GetWindowTextW returned no parent title",
        );
    }
    if class_len == 0 {
        log_diagnostic_record(
            "video-window",
            root_call_seq,
            "event=error GetClassNameW returned no parent class name",
        );
    }
    log_diagnostic_record(
        "video-window",
        root_call_seq,
        format_args!(
            "event=parent-identity title={} class={}",
            format_win32_text(&title_buffer, title_len),
            format_win32_text(&class_buffer, class_len),
        ),
    );
}

#[cfg(windows)]
fn log_video_window_geometry(
    parent: windows::Win32::Foundation::HWND,
    child: windows::Win32::Foundation::HWND,
    expected: (i32, i32, i32, i32),
    root_call_seq: u64,
) {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

    let mut parent_client = RECT::default();
    let mut parent_window = RECT::default();
    let mut child_client = RECT::default();
    let mut child_window = RECT::default();
    let parent_client_result = unsafe { GetClientRect(parent, &mut parent_client) };
    let parent_window_result = unsafe { GetWindowRect(parent, &mut parent_window) };
    let child_client_result = unsafe { GetClientRect(child, &mut child_client) };
    let child_window_result = unsafe { GetWindowRect(child, &mut child_window) };
    let (child_in_parent, actual_matches_expected) = match child_window_result.as_ref() {
        Ok(_) => {
            let mut corners = [
                POINT {
                    x: child_window.left,
                    y: child_window.top,
                },
                POINT {
                    x: child_window.right,
                    y: child_window.bottom,
                },
            ];
            let top_left = unsafe { ScreenToClient(parent, &mut corners[0]) }.as_bool();
            let bottom_right = unsafe { ScreenToClient(parent, &mut corners[1]) }.as_bool();
            if top_left && bottom_right {
                let actual = (
                    corners[0].x,
                    corners[0].y,
                    corners[1].x - corners[0].x,
                    corners[1].y - corners[0].y,
                );
                let rect = format!(
                    "({},{}..{},{} size={}x{})",
                    corners[0].x,
                    corners[0].y,
                    corners[1].x,
                    corners[1].y,
                    corners[1].x - corners[0].x,
                    corners[1].y - corners[0].y,
                );
                (rect, actual == expected)
            } else {
                log_diagnostic_record(
                    "video-window",
                    root_call_seq,
                    format_args!(
                        "event=error screen-to-client top_left={top_left} \
                         bottom_right={bottom_right}"
                    ),
                );
                ("conversion-error".to_owned(), false)
            }
        }
        Err(_) => ("unavailable".to_owned(), false),
    };
    log_diagnostic_record(
        "video-window",
        root_call_seq,
        format_args!(
            "event=native-geometry parent_client_px={} parent_window_screen_px={} \
             child_client_px={} child_window_screen_px={} \
             child_in_parent_client_px={} expected_set_window_pos_px={expected:?} \
             child_geometry_matches_expected={actual_matches_expected}",
            format_win32_rect(parent_client_result.map(|()| parent_client)),
            format_win32_rect(parent_window_result.map(|()| parent_window)),
            format_win32_rect(child_client_result.map(|()| child_client)),
            format_win32_rect(child_window_result.map(|()| child_window)),
            child_in_parent,
        ),
    );
}

#[cfg(windows)]
fn format_win32_rect(result: windows::core::Result<windows::Win32::Foundation::RECT>) -> String {
    match result {
        Ok(rect) => format!(
            "({},{}..{},{} size={}x{})",
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            rect.right - rect.left,
            rect.bottom - rect.top
        ),
        Err(error) => format!("error({error})"),
    }
}

#[cfg(windows)]
fn format_win32_text(buffer: &[u16], length: i32) -> String {
    if length <= 0 {
        return "<unavailable>".to_owned();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

#[cfg(windows)]
fn set_video_output_visible(child: usize, visible: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow};

    unsafe {
        ShowWindow(
            HWND(child as *mut std::ffi::c_void),
            if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
        );
    }
}

#[cfg(not(windows))]
fn set_video_output_visible(_child: usize, _visible: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str) -> Track {
        Track {
            path: PathBuf::from(path),
            title: path.to_owned(),
            artist: None,
            album: None,
            duration: None,
            media_type: MediaType::Audio,
        }
    }

    #[test]
    fn playlist_current_tracks_the_loaded_path_not_the_selection() {
        let tracks = [track("first.mp3"), track("second.mp3")];

        assert_eq!(
            playlist_current_for_path(&tracks, Some(0), Some(&tracks[1].path)),
            Some(1)
        );
        assert_eq!(playlist_current_for_path(&tracks, Some(0), None), None);
    }

    #[test]
    fn playlist_current_preserves_the_matching_duplicate_index() {
        let tracks = [track("same.mp3"), track("same.mp3")];

        assert_eq!(
            playlist_current_for_path(&tracks, Some(1), Some(&tracks[0].path)),
            Some(1)
        );
    }

    #[test]
    fn natural_end_moves_to_next_track_without_wrapping() {
        assert_eq!(natural_end_next_index(Some(0), 3), Some(1));
        assert_eq!(natural_end_next_index(Some(1), 3), Some(2));
        assert_eq!(natural_end_next_index(Some(2), 3), None);
        assert_eq!(natural_end_next_index(None, 3), None);
        assert_eq!(natural_end_next_index(Some(3), 3), None);
    }

    #[test]
    fn confirmed_video_result_matches_the_active_request() {
        let result = VideoPlayResult {
            request_id: 7,
            path: PathBuf::from("video.mp4"),
        };

        assert_eq!(
            confirmed_video_path(7, &result),
            Some(PathBuf::from("video.mp4"))
        );
        assert_eq!(confirmed_video_path(8, &result), None);
    }

    #[test]
    fn video_viewport_close_resets_all_local_video_state() {
        let mut video_open = true;
        let mut video_visible = true;
        let mut video_fullscreen = true;
        let mut pending_video_start = Some((PathBuf::from("video.mp4"), 7));
        let mut video_request_id = Some(7);

        reset_video_viewport_state(
            &mut video_open,
            &mut video_visible,
            &mut video_fullscreen,
            &mut pending_video_start,
            &mut video_request_id,
        );

        assert!(!video_open);
        assert!(!video_visible);
        assert!(!video_fullscreen);
        assert!(pending_video_start.is_none());
        assert!(video_request_id.is_none());
    }

    #[test]
    fn video_output_stays_below_controls_in_windowed_mode() {
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(720.0, 405.0));

        assert_eq!(
            video_output_rect(viewport, false),
            egui::Rect::from_min_max(egui::pos2(6.0, 18.0), egui::pos2(714.0, 381.0))
        );
        assert_eq!(video_output_rect(viewport, true), viewport);
    }

    #[test]
    fn viewport_diagnostics_sample_baseline_transitions_and_three_restore_frames() {
        let normal = ViewportDiagnosticState {
            minimized: Some(false),
            visible: Some(true),
            focused: Some(true),
            fullscreen: Some(false),
        };
        let minimized = ViewportDiagnosticState {
            minimized: Some(true),
            visible: Some(false),
            ..normal.clone()
        };
        let mut previous = None;
        let mut restore_frames_remaining = 0;

        assert_eq!(
            update_diagnostic_sample(normal.clone(), &mut previous, &mut restore_frames_remaining),
            (true, "baseline")
        );
        assert_eq!(
            update_diagnostic_sample(normal.clone(), &mut previous, &mut restore_frames_remaining),
            (false, "unchanged")
        );
        assert_eq!(
            update_diagnostic_sample(minimized, &mut previous, &mut restore_frames_remaining),
            (true, "minimized-or-not-render-visible")
        );
        assert_eq!(
            update_diagnostic_sample(normal.clone(), &mut previous, &mut restore_frames_remaining),
            (true, "restored")
        );
        for _ in 0..3 {
            assert_eq!(
                update_diagnostic_sample(
                    normal.clone(),
                    &mut previous,
                    &mut restore_frames_remaining
                ),
                (true, "post-restore")
            );
        }
        assert_eq!(
            update_diagnostic_sample(normal, &mut previous, &mut restore_frames_remaining),
            (false, "unchanged")
        );
    }

    #[test]
    fn pause_action_routes_pending_delay_and_loading_to_pause_request() {
        for phase in [
            PlayRequestPhase::PendingDelay(41),
            PlayRequestPhase::Loading(41),
        ] {
            let snapshot = PlaybackSnapshot {
                play_request_phase: phase,
                path: None,
                state: PlaybackState::Loading,
                ..PlaybackSnapshot::default()
            };
            assert_eq!(pause_action(&snapshot), PauseAction::Pause(Some(41)));
        }
    }

    #[test]
    fn pause_action_does_not_turn_pending_play_into_play() {
        let snapshot = PlaybackSnapshot {
            play_request_phase: PlayRequestPhase::PendingDelay(41),
            path: Some(PathBuf::from("old.mp3")),
            state: PlaybackState::Playing,
            ..PlaybackSnapshot::default()
        };
        assert_eq!(pause_action(&snapshot), PauseAction::Pause(Some(41)));
    }

    #[test]
    fn pause_action_preserves_active_and_no_request_behavior() {
        let active = PlaybackSnapshot {
            play_request_phase: PlayRequestPhase::Active(41),
            path: Some(PathBuf::from("active.mp3")),
            state: PlaybackState::Playing,
            ..PlaybackSnapshot::default()
        };
        assert_eq!(pause_action(&active), PauseAction::Pause(None));

        let stopped = PlaybackSnapshot::default();
        assert_eq!(pause_action(&stopped), PauseAction::Play);
    }
}

#[cfg(windows)]
fn find_video_window() -> usize {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::PCWSTR;

    let title: Vec<u16> = VIDEO_TITLE.encode_utf16().chain(Some(0)).collect();
    unsafe { FindWindowW(None, PCWSTR(title.as_ptr())) }
        .map(|window| window.0 as usize)
        .unwrap_or_default()
}

#[cfg(not(windows))]
fn find_video_window() -> usize {
    0
}

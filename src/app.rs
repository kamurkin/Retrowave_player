use crate::media::{MediaType, Track, format_time};
use crate::playback::{Playback, PlaybackSnapshot, PlaybackState, TrackMetadata};
use crate::playlist::Playlist;
use crate::skin;
use eframe::egui::{
    self, Align2, Color32, FontId, Frame, Margin, RichText, Stroke, ViewportBuilder, ViewportId,
    pos2, vec2,
};
use std::path::PathBuf;
use std::time::Duration;

const VIDEO_TITLE: &str = "RetroWave Video";
const BANDS: [&str; 10] = [
    "60", "170", "310", "600", "1K", "3K", "6K", "12K", "14K", "16K",
];

fn playlist_viewport() -> ViewportId {
    ViewportId::from_hash_of("retrowave-playlist")
}

fn equalizer_viewport() -> ViewportId {
    ViewportId::from_hash_of("retrowave-equalizer")
}

fn video_viewport() -> ViewportId {
    ViewportId::from_hash_of("retrowave-video")
}

pub struct RetroWaveApp {
    playback: Playback,
    snapshot: PlaybackSnapshot,
    playlist: Playlist,
    playlist_open: bool,
    equalizer_open: bool,
    video_open: bool,
    video_fullscreen: bool,
    video_target: usize,
    pending_video: Option<PathBuf>,
    volume: u8,
    muted: bool,
    rate: f32,
    equalizer_enabled: bool,
    preamp: f32,
    bands: [f32; 10],
    seek_value: f64,
    notice: Option<String>,
}

impl RetroWaveApp {
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        skin::apply(&context.egui_ctx);
        Self {
            playback: Playback::start(),
            snapshot: PlaybackSnapshot::default(),
            playlist: Playlist::default(),
            playlist_open: false,
            equalizer_open: false,
            video_open: false,
            video_fullscreen: false,
            video_target: 0,
            pending_video: None,
            volume: 70,
            muted: false,
            rate: 1.0,
            equalizer_enabled: false,
            preamp: 0.0,
            bands: [0.0; 10],
            seek_value: 0.0,
            notice: None,
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
        self.playlist.current = Some(index);
        self.playlist.selected = Some(index);
        self.notice = None;
        if is_video {
            self.video_open = true;
            if self.video_target == 0 {
                self.pending_video = Some(path);
            } else {
                self.playback.play(path, self.video_target);
            }
        } else {
            self.pending_video = None;
            self.video_open = false;
            self.video_target = 0;
            self.playback.set_video_target(0);
            self.playback.play(path, 0);
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

    fn show_player(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing = vec2(4.0, 3.0);
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
                ui.add_space(2.0);
                self.show_seek(ui);
                ui.add_space(2.0);
                self.show_transport(ui);
                self.show_playback_options(ui);
                if let Some(error) = &self.snapshot.error {
                    ui.label(RichText::new(error).color(skin::LCD_RED).size(8.0));
                } else if let Some(notice) = &self.notice {
                    ui.label(RichText::new(notice).color(skin::LCD_YELLOW).size(8.0));
                } else if !self.snapshot.ready {
                    ui.label(
                        RichText::new("INITIALIZING LIBVLC")
                            .color(skin::LCD_DIM)
                            .size(7.0),
                    );
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
        if self.snapshot.position.is_some() {
            self.seek_value = self.snapshot.position.unwrap().as_secs_f64();
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
                if self.snapshot.path.is_none() || self.snapshot.state == PlaybackState::Stopped {
                    self.play_selected();
                } else {
                    self.playback.pause();
                }
            }
            if skin::transport_button(ui, skin::TransportIcon::Stop, false).clicked() {
                self.playback.stop();
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
                && skin::text_button(ui, "VID", vec2(28.0, 18.0), self.video_open, true).clicked()
            {
                self.toggle_video();
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

    fn toggle_video(&mut self) {
        self.video_open = !self.video_open;
        if !self.video_open {
            self.video_target = 0;
            self.playback.set_video_target(0);
        }
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
                    self.playback.stop();
                    self.playback.set_video_target(0);
                    self.video_target = 0;
                    self.video_open = false;
                    self.pending_video = None;
                }
            }
            if skin::text_button(ui, "CLEAR", vec2(42.0, 18.0), false, true).clicked() {
                self.playlist.clear();
                self.playback.stop();
                self.playback.set_video_target(0);
                self.video_target = 0;
                self.video_open = false;
                self.pending_video = None;
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

    fn show_video(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        if context.input(|input| input.key_pressed(egui::Key::Escape)) && self.video_fullscreen {
            self.video_fullscreen = false;
            context.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        let close_requested = context.input(|input| input.viewport().close_requested());
        if close_requested {
            self.video_open = false;
            self.video_fullscreen = false;
            self.video_target = 0;
            self.pending_video = None;
            self.playback.set_video_target(0);
            context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }

        ui.painter().rect_filled(ui.max_rect(), 0.0, Color32::BLACK);
        let mut close_video = false;
        if !self.video_fullscreen {
            let title = skin::title_bar(ui, "RETROWAVE VIDEO", &[("X", false)]);
            if title.drag_started {
                context.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            close_video = title.clicked == Some(0);
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    if skin::text_button(ui, "FULLSCREEN", vec2(64.0, 18.0), false, true).clicked()
                    {
                        self.video_fullscreen = true;
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    }
                    ui.label(
                        RichText::new("VIDEO // ESC EXITS FULLSCREEN")
                            .color(skin::TEXT_DIM)
                            .monospace()
                            .size(8.0),
                    );
                });
            });
        }
        if close_video {
            self.video_open = false;
            self.video_target = 0;
            self.pending_video = None;
            self.playback.set_video_target(0);
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let target = find_video_window();
        if target != self.video_target {
            self.video_target = target;
            if target == 0 {
                self.playback.set_video_target(0);
            } else if let Some(path) = self.pending_video.take() {
                self.playback.play(path, target);
            } else {
                self.playback.set_video_target(target);
            }
        } else if target != 0 {
            if let Some(path) = self.pending_video.take() {
                self.playback.play(path, target);
            }
        }
    }

    fn render_child_windows(&mut self, context: &egui::Context) {
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
        if self.video_open {
            let close_requested = std::cell::Cell::new(false);
            context.show_viewport_immediate(
                video_viewport(),
                ViewportBuilder::default()
                    .with_title(VIDEO_TITLE)
                    .with_inner_size([720.0, 405.0])
                    .with_min_inner_size([320.0, 180.0])
                    .with_decorations(false)
                    .with_fullscreen(self.video_fullscreen),
                |ui, _| {
                    close_requested.set(ui.ctx().input(|input| input.viewport().close_requested()));
                    self.show_video(ui);
                },
            );
            if close_requested.get() {
                self.video_open = false;
                self.video_target = 0;
                self.playback.set_video_target(0);
            }
        }
    }
}

impl eframe::App for RetroWaveApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let context = root_ui.ctx().clone();
        self.snapshot = self.playback.snapshot();
        self.update_metadata();
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
            .show(root_ui, |ui| self.show_player(ui));
        self.render_child_windows(&context);
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

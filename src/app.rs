use crate::media::{MediaType, Track, format_time};
use crate::playback::{Playback, PlaybackSnapshot, PlaybackState, TrackMetadata};
use crate::playlist::Playlist;
use crate::skin;
use eframe::egui::{self, Color32, RichText, Stroke, Vec2, ViewportBuilder, ViewportId};
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
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("RETROWAVE").color(skin::GOLD).strong());
            ui.label(RichText::new("PLAYER").color(skin::TEXT));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("X").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if ui.small_button("_").clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
            });
        });
        ui.add_space(3.0);

        egui::Frame::new()
            .fill(skin::PANEL)
            .stroke(Stroke::new(1.0, skin::RAISED))
            .inner_margin(egui::Margin::same(7))
            .show(ui, |ui| {
                let current = self
                    .playlist
                    .current
                    .and_then(|index| self.playlist.tracks.get(index));
                let title = current.map_or("NO TRACK LOADED", |track| track.title.as_str());
                let artist = current
                    .and_then(|track| track.artist.as_deref())
                    .unwrap_or("RETROWAVE PLAYER");
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(title).color(skin::LCD).strong().size(15.0));
                        ui.label(RichText::new(artist).color(skin::LCD).size(11.0));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} / {}",
                                format_time(self.snapshot.position),
                                format_time(self.snapshot.duration)
                            ))
                            .color(skin::LCD)
                            .monospace()
                            .size(15.0),
                        );
                    });
                });
                ui.add_space(5.0);
                ui.label(
                    RichText::new(self.visualizer_status())
                        .color(skin::MUTED)
                        .small(),
                );
            });

        let max_seek = self
            .snapshot
            .duration
            .map(|duration| duration.as_secs_f64())
            .unwrap_or(1.0)
            .max(1.0);
        if self.snapshot.position.is_some() {
            self.seek_value = self.snapshot.position.unwrap().as_secs_f64();
        }
        let seek_response = ui.add_enabled(
            self.snapshot.duration.is_some() && self.snapshot.path.is_some(),
            egui::Slider::new(&mut self.seek_value, 0.0..=max_seek)
                .show_value(false)
                .text(""),
        );
        if seek_response.drag_stopped() || seek_response.clicked() {
            self.playback.seek(Duration::from_secs_f64(
                self.seek_value.clamp(0.0, max_seek),
            ));
        }

        ui.horizontal(|ui| {
            if ui.button("|<").on_hover_text("Previous").clicked() {
                self.play_next(-1);
            }
            if ui.button("▶").on_hover_text("Play").clicked() {
                self.play_selected();
            }
            if ui.button("||").on_hover_text("Pause / resume").clicked() {
                if self.snapshot.path.is_none() || self.snapshot.state == PlaybackState::Stopped {
                    self.play_selected();
                } else {
                    self.playback.pause();
                }
            }
            if ui.button("[]").on_hover_text("Stop").clicked() {
                self.playback.stop();
            }
            if ui.button(">|").on_hover_text("Next").clicked() {
                self.play_next(1);
            }
            ui.separator();
            if ui
                .add(egui::Slider::new(&mut self.volume, 0..=100).show_value(false))
                .changed()
            {
                self.playback.set_volume(self.volume);
            }
            if ui
                .selectable_label(self.muted, "MUTE")
                .on_hover_text("Mute")
                .clicked()
            {
                self.muted = !self.muted;
                self.playback.set_muted(self.muted);
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("SPEED").color(skin::MUTED));
            egui::ComboBox::from_id_salt("playback-speed")
                .selected_text(format!("{:.2}x", self.rate))
                .width(70.0)
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
            if ui
                .selectable_label(self.playlist_open, "PL")
                .on_hover_text("Playlist editor")
                .clicked()
            {
                self.playlist_open = !self.playlist_open;
            }
            if ui
                .selectable_label(self.equalizer_open, "EQ")
                .on_hover_text("Equalizer")
                .clicked()
            {
                self.equalizer_open = !self.equalizer_open;
            }
            let video_loaded = self
                .playlist
                .current
                .and_then(|index| self.playlist.tracks.get(index))
                .is_some_and(|track| track.media_type == MediaType::Video);
            if ui
                .add_enabled(video_loaded || self.video_open, egui::Button::new("VIDEO"))
                .clicked()
            {
                self.video_open = !self.video_open;
                if !self.video_open {
                    self.video_target = 0;
                    self.playback.set_video_target(0);
                }
            }
            ui.label(RichText::new(self.playlist.summary()).color(skin::MUTED));
        });

        ui.horizontal(|ui| {
            if !self.snapshot.audio_tracks.is_empty() {
                egui::ComboBox::from_id_salt("audio-track")
                    .selected_text("AUDIO")
                    .show_ui(ui, |ui| {
                        let tracks = self.snapshot.audio_tracks.clone();
                        for track in tracks {
                            if ui.button(&track.label).clicked() {
                                self.playback.set_audio_track(track.id);
                            }
                        }
                    });
            }
            if !self.snapshot.subtitles.is_empty() {
                egui::ComboBox::from_id_salt("subtitle-track")
                    .selected_text("SUBTITLE")
                    .show_ui(ui, |ui| {
                        let tracks = self.snapshot.subtitles.clone();
                        for track in tracks {
                            if ui.button(&track.label).clicked() {
                                self.playback.set_subtitle(track.id);
                            }
                        }
                        if ui.button("Off").clicked() {
                            self.playback.set_subtitle(-1);
                        }
                    });
            }
            if ui.button("ADD SRT/ASS").clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Subtitles", &["srt", "ass"])
                    .pick_file()
                {
                    self.playback.add_subtitle(path);
                }
            }
        });

        if let Some(error) = &self.snapshot.error {
            ui.label(RichText::new(error).color(Color32::from_rgb(255, 104, 85)));
        } else if let Some(notice) = &self.notice {
            ui.label(RichText::new(notice).color(Color32::from_rgb(255, 190, 90)));
        } else if !self.snapshot.ready {
            ui.label(
                RichText::new("INITIALIZING LIBVLC…")
                    .color(skin::GOLD)
                    .small(),
            );
        }
    }

    fn visualizer_status(&self) -> &'static str {
        match (self.snapshot.state, self.snapshot.path.is_some()) {
            (PlaybackState::Playing, true) => "AUDIO VISUALIZER  •  PCM TAP UNAVAILABLE",
            (PlaybackState::Loading, true) => "OPENING MEDIA",
            (PlaybackState::Paused, true) => "PLAYBACK PAUSED",
            (_, false) => "NO AUDIO SIGNAL",
            _ => "READY",
        }
    }

    fn show_playlist(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("PLAYLIST EDITOR").color(skin::GOLD).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("X").clicked() {
                    self.playlist_open = false;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("ADD FILES").clicked() {
                self.choose_files();
            }
            if ui.button("REMOVE").clicked() {
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
            if ui.button("CLEAR").clicked() {
                self.playlist.clear();
                self.playback.stop();
                self.playback.set_video_target(0);
                self.video_target = 0;
                self.video_open = false;
                self.pending_video = None;
            }
        });
        ui.add_space(4.0);
        egui::Frame::new()
            .fill(skin::PANEL)
            .stroke(Stroke::new(1.0, skin::RAISED))
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                egui::Grid::new("playlist-header")
                    .num_columns(3)
                    .striped(false)
                    .show(ui, |ui| {
                        ui.label(RichText::new("TITLE").color(skin::GOLD));
                        ui.label(RichText::new("TYPE").color(skin::GOLD));
                        ui.label(RichText::new("LENGTH").color(skin::GOLD));
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
                                    skin::LCD
                                } else {
                                    skin::TEXT
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
        ui.label(RichText::new(self.playlist.summary()).color(skin::LCD));
        ui.label(
            RichText::new("Drop MP3 / FLAC / WAV / MP4 / MKV / AVI files here.")
                .color(skin::MUTED)
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
        ui.horizontal(|ui| {
            ui.label(RichText::new("EQUALIZER").color(skin::GOLD).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("X").clicked() {
                    self.equalizer_open = false;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
        ui.separator();
        if self.snapshot.equalizer_supported {
            ui.label(RichText::new("DSP: LIBVLC EQUALIZER").color(skin::LCD));
        } else {
            ui.label(
                RichText::new("DSP NOT AVAILABLE IN THE LOADED VLC RUNTIME")
                    .color(Color32::from_rgb(255, 160, 90)),
            );
        }
        let mut changed = false;
        ui.add_enabled_ui(self.snapshot.equalizer_supported, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .selectable_label(self.equalizer_enabled, "EQ ON")
                    .clicked()
                {
                    self.equalizer_enabled = !self.equalizer_enabled;
                    changed = true;
                }
                ui.label("PREAMP");
                changed |= ui
                    .add(egui::Slider::new(&mut self.preamp, -20.0..=20.0).show_value(true))
                    .changed();
            });
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                for (index, band) in self.bands.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        changed |= ui
                            .add(
                                egui::Slider::new(band, -20.0..=20.0)
                                    .vertical()
                                    .show_value(false),
                            )
                            .changed();
                        ui.label(RichText::new(BANDS[index]).color(skin::MUTED).small());
                    });
                }
            });
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.snapshot.equalizer_supported,
                    egui::Button::new("FLAT / RESET"),
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
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.horizontal(|ui| {
                    if ui.button("FULLSCREEN").clicked() {
                        self.video_fullscreen = true;
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                    }
                    if ui.button("CLOSE VIDEO").clicked() {
                        close_video = true;
                    }
                    ui.label(
                        RichText::new("ESC exits fullscreen")
                            .color(skin::TEXT)
                            .small(),
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
                    .with_inner_size([500.0, 340.0])
                    .with_min_inner_size([380.0, 220.0]),
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
                    .with_inner_size([400.0, 250.0])
                    .with_min_inner_size([350.0, 220.0]),
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

        egui::Frame::new()
            .fill(skin::FRAME)
            .inner_margin(egui::Margin::same(8))
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

use libloading::Library;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;

pub fn configure_vlc_plugin_path() {
    if std::env::var_os("VLC_PLUGIN_PATH").is_some() {
        return;
    }

    for path in vlc_library_candidates() {
        if !path.is_file() {
            continue;
        }
        let Some(plugin_path) = path
            .parent()
            .map(|parent| parent.join("plugins"))
            .filter(|plugin_path| plugin_path.is_dir())
        else {
            continue;
        };
        // Called from main before eframe starts any worker threads.
        unsafe {
            std::env::set_var("VLC_PLUGIN_PATH", plugin_path);
        }
        return;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Loading,
    Playing,
    Paused,
}

#[derive(Clone, Debug, Default)]
pub struct TrackMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
}

#[derive(Clone, Debug, Default)]
pub struct TrackOption {
    pub id: i32,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct PlaybackSnapshot {
    pub ready: bool,
    pub state: PlaybackState,
    pub path: Option<PathBuf>,
    pub position: Option<Duration>,
    pub duration: Option<Duration>,
    pub volume: u8,
    pub muted: bool,
    pub rate: f32,
    pub error: Option<String>,
    pub metadata: HashMap<PathBuf, TrackMetadata>,
    pub audio_tracks: Vec<TrackOption>,
    pub subtitles: Vec<TrackOption>,
    pub equalizer_supported: bool,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            ready: false,
            state: PlaybackState::Stopped,
            path: None,
            position: None,
            duration: None,
            volume: 70,
            muted: false,
            rate: 1.0,
            error: None,
            metadata: HashMap::new(),
            audio_tracks: Vec::new(),
            subtitles: Vec::new(),
            equalizer_supported: false,
        }
    }
}

enum Command {
    Play(PathBuf, usize),
    Pause,
    Stop,
    Seek(Duration),
    Volume(u8),
    Mute(bool),
    Rate(f32),
    SetVideoTarget(usize),
    Probe(Vec<PathBuf>),
    AddSubtitle(PathBuf),
    AudioTrack(i32),
    Subtitle(i32),
    Equalizer(bool, f32, [f32; 10]),
}

pub struct Playback {
    sender: Sender<Command>,
    snapshot: Arc<Mutex<PlaybackSnapshot>>,
}

impl Playback {
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(PlaybackSnapshot::default()));
        let worker_snapshot = Arc::clone(&snapshot);
        if let Err(error) = thread::Builder::new()
            .name("retrowave-libvlc".to_owned())
            .spawn(move || playback_worker(receiver, worker_snapshot))
        {
            update_snapshot(&snapshot, |state| {
                state.error = Some(format!("Could not start playback worker: {error}"));
            });
        }

        Self { sender, snapshot }
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        self.snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn play(&self, path: PathBuf, video_target: usize) {
        self.send(Command::Play(path, video_target));
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    pub fn seek(&self, time: Duration) {
        self.send(Command::Seek(time));
    }

    pub fn set_volume(&self, volume: u8) {
        self.send(Command::Volume(volume));
    }

    pub fn set_muted(&self, muted: bool) {
        self.send(Command::Mute(muted));
    }

    pub fn set_rate(&self, rate: f32) {
        self.send(Command::Rate(rate));
    }

    pub fn set_video_target(&self, target: usize) {
        self.send(Command::SetVideoTarget(target));
    }

    pub fn probe(&self, paths: Vec<PathBuf>) {
        if !paths.is_empty() {
            self.send(Command::Probe(paths));
        }
    }

    pub fn add_subtitle(&self, path: PathBuf) {
        self.send(Command::AddSubtitle(path));
    }

    pub fn set_audio_track(&self, id: i32) {
        self.send(Command::AudioTrack(id));
    }

    pub fn set_subtitle(&self, id: i32) {
        self.send(Command::Subtitle(id));
    }

    pub fn set_equalizer(&self, enabled: bool, preamp: f32, bands: [f32; 10]) {
        self.send(Command::Equalizer(enabled, preamp, bands));
    }

    fn send(&self, command: Command) {
        if let Err(error) = self.sender.send(command) {
            let mut snapshot = self
                .snapshot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if snapshot.error.is_none() {
                snapshot.error = Some(format!("Playback worker is unavailable: {error}"));
            }
        }
    }
}

fn playback_worker(receiver: Receiver<Command>, snapshot: Arc<Mutex<PlaybackSnapshot>>) {
    let mut engine = match VlcEngine::new() {
        Ok(engine) => engine,
        Err(error) => {
            update_snapshot(&snapshot, |state| state.error = Some(error));
            return;
        }
    };
    update_snapshot(&snapshot, |state| {
        state.ready = true;
        state.equalizer_supported = engine.api.equalizer_new.is_some()
            && engine.api.equalizer_set_preamp.is_some()
            && engine.api.equalizer_set_band.is_some()
            && engine.api.player_set_equalizer.is_some();
        state.error = None;
    });

    let mut probes = Vec::<Probe>::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => {
                if process_command(command, &mut engine, &snapshot, &mut probes) {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        engine.refresh(&snapshot);
        poll_probes(&mut engine, &snapshot, &mut probes);
    }
}

fn process_command(
    command: Command,
    engine: &mut VlcEngine,
    snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    probes: &mut Vec<Probe>,
) -> bool {
    let result = match command {
        Command::Play(path, target) => engine.play(path, target),
        Command::Pause => engine.pause(),
        Command::Stop => engine.stop(),
        Command::Seek(time) => engine.seek(time),
        Command::Volume(volume) => engine.set_volume(volume),
        Command::Mute(muted) => engine.set_muted(muted),
        Command::Rate(rate) => engine.set_rate(rate),
        Command::SetVideoTarget(target) => {
            engine.set_video_target(target);
            Ok(())
        }
        Command::Probe(paths) => {
            engine.schedule_probes(paths, probes, snapshot);
            Ok(())
        }
        Command::AddSubtitle(path) => engine.add_subtitle(path),
        Command::AudioTrack(id) => engine.set_audio_track(id),
        Command::Subtitle(id) => engine.set_subtitle(id),
        Command::Equalizer(enabled, preamp, bands) => engine.set_equalizer(enabled, preamp, bands),
    };
    match result {
        Ok(()) => update_snapshot(snapshot, |state| {
            state.error = None;
            state.volume = engine.volume;
            state.muted = engine.muted;
            state.rate = engine.rate;
        }),
        Err(error) => update_snapshot(snapshot, |state| state.error = Some(error)),
    }
    false
}

fn update_snapshot(
    snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    update: impl FnOnce(&mut PlaybackSnapshot),
) {
    update(
        &mut snapshot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
}

struct Probe {
    path: PathBuf,
    media: *mut c_void,
    deadline: Instant,
}

struct VlcEngine {
    api: VlcApi,
    instance: *mut c_void,
    player: *mut c_void,
    media: *mut c_void,
    equalizer: *mut c_void,
    video_target: usize,
    volume: u8,
    muted: bool,
    rate: f32,
    state: PlaybackState,
    current_path: Option<PathBuf>,
    equalizer_settings: (bool, f32, [f32; 10]),
    last_track_poll: Instant,
}

impl VlcEngine {
    fn new() -> Result<Self, String> {
        let (library, library_path, plugin_path) = load_vlc_library()?;
        // The API table stores copied function pointers; retaining Library keeps them valid.
        let api = unsafe { VlcApi::load(library)? };
        plugin_path.ok_or_else(|| {
            format!(
                "The VLC plugins directory was not found for {}.",
                library_path.display()
            )
        })?;
        let arguments = [CString::new("--no-video-title-show").unwrap()];
        let argument_ptrs: Vec<*const c_char> =
            arguments.iter().map(|argument| argument.as_ptr()).collect();
        let instance =
            unsafe { (api.instance_new)(argument_ptrs.len() as c_int, argument_ptrs.as_ptr()) };
        if instance.is_null() {
            return Err(format!(
                "Could not initialize libVLC from {}. Verify that VLC is installed and its plugin directory is available.",
                library_path.display()
            ));
        }
        let player = unsafe { (api.player_new)(instance) };
        if player.is_null() {
            unsafe { (api.instance_release)(instance) };
            return Err("libVLC could not create a media player.".to_owned());
        }

        let mut engine = Self {
            api,
            instance,
            player,
            media: ptr::null_mut(),
            equalizer: ptr::null_mut(),
            video_target: 0,
            volume: 70,
            muted: false,
            rate: 1.0,
            state: PlaybackState::Stopped,
            current_path: None,
            equalizer_settings: (false, 0.0, [0.0; 10]),
            last_track_poll: Instant::now(),
        };
        engine.set_volume(70)?;
        Ok(engine)
    }

    fn play(&mut self, path: PathBuf, video_target: usize) -> Result<(), String> {
        if !path.is_file() {
            return Err(format!(
                "Media file is no longer available: {}",
                path.display()
            ));
        }
        let path_text = path
            .to_str()
            .ok_or_else(|| format!("Media path is not valid Unicode: {}", path.display()))?;
        let path_c = CString::new(path_text)
            .map_err(|_| format!("Media path contains a null character: {}", path.display()))?;
        let media = unsafe { (self.api.media_new_path)(self.instance, path_c.as_ptr()) };
        if media.is_null() {
            return Err(format!(
                "libVLC could not open {}: {}",
                path.display(),
                self.last_error()
            ));
        }
        self.stop_current();
        unsafe {
            (self.api.player_set_media)(self.player, media);
            (self.api.media_parse_async)(media);
        }
        self.media = media;
        self.current_path = Some(path.clone());
        self.video_target = video_target;
        unsafe { (self.api.player_set_hwnd)(self.player, video_target as *mut c_void) };
        if (unsafe { (self.api.player_play)(self.player) }) != 0 {
            self.stop_current();
            return Err(format!(
                "libVLC failed to start {}: {}",
                path.display(),
                self.last_error()
            ));
        }
        self.state = PlaybackState::Loading;
        self.apply_audio_settings()?;
        append_history(&path)?;
        Ok(())
    }

    fn pause(&mut self) -> Result<(), String> {
        if self.media.is_null() {
            return Err("Load a track before using Pause.".to_owned());
        }
        let next = if self.state == PlaybackState::Playing {
            PlaybackState::Paused
        } else {
            PlaybackState::Playing
        };
        unsafe {
            (self.api.player_set_pause)(self.player, i32::from(next == PlaybackState::Paused))
        };
        self.state = next;
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        if !self.media.is_null() {
            unsafe { (self.api.player_stop)(self.player) };
        }
        self.state = PlaybackState::Stopped;
        Ok(())
    }

    fn stop_current(&mut self) {
        if !self.media.is_null() {
            unsafe {
                (self.api.player_stop)(self.player);
                (self.api.player_set_media)(self.player, ptr::null_mut());
                (self.api.media_release)(self.media);
            }
            self.media = ptr::null_mut();
        }
        self.state = PlaybackState::Stopped;
    }

    fn seek(&mut self, time: Duration) -> Result<(), String> {
        if self.media.is_null() {
            return Err("Load a track before seeking.".to_owned());
        }
        let millis = time.as_millis().min(i64::MAX as u128) as i64;
        unsafe { (self.api.player_set_time)(self.player, millis) };
        Ok(())
    }

    fn set_volume(&mut self, volume: u8) -> Result<(), String> {
        let result = unsafe { (self.api.audio_set_volume)(self.player, i32::from(volume)) };
        if result != 0 {
            return Err(format!(
                "libVLC could not set volume: {}",
                self.last_error()
            ));
        }
        self.volume = volume;
        Ok(())
    }

    fn set_muted(&mut self, muted: bool) -> Result<(), String> {
        unsafe { (self.api.audio_set_mute)(self.player, i32::from(muted)) };
        self.muted = muted;
        Ok(())
    }

    fn set_rate(&mut self, rate: f32) -> Result<(), String> {
        let result = unsafe { (self.api.player_set_rate)(self.player, rate) };
        if result != 0 {
            return Err(format!(
                "libVLC does not support playback speed {rate:.2}x for this media."
            ));
        }
        self.rate = rate;
        Ok(())
    }

    fn set_video_target(&mut self, target: usize) {
        self.video_target = target;
        unsafe { (self.api.player_set_hwnd)(self.player, target as *mut c_void) };
    }

    fn add_subtitle(&mut self, path: PathBuf) -> Result<(), String> {
        let Some(add_slave) = self.api.player_add_slave else {
            return Err("This VLC runtime does not expose external subtitle support.".to_owned());
        };
        if !path.is_file() {
            return Err(format!("Subtitle file does not exist: {}", path.display()));
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != "srt" && extension != "ass" {
            return Err("Subtitle files must use the .srt or .ass format.".to_owned());
        }
        let url = Url::from_file_path(&path)
            .map_err(|_| format!("Could not create a file URL for {}", path.display()))?;
        let url = CString::new(url.as_str())
            .map_err(|_| "Subtitle path contains a null character.".to_owned())?;
        let result = unsafe { add_slave(self.player, 1, url.as_ptr(), 1) };
        if result != 0 {
            return Err(format!(
                "libVLC could not load subtitle: {}",
                self.last_error()
            ));
        }
        Ok(())
    }

    fn set_audio_track(&mut self, id: i32) -> Result<(), String> {
        let Some(set_track) = self.api.audio_set_track else {
            return Err("This VLC runtime does not expose audio track selection.".to_owned());
        };
        if unsafe { set_track(self.player, id) } != 0 {
            return Err(format!(
                "Could not select the audio track: {}",
                self.last_error()
            ));
        }
        Ok(())
    }

    fn set_subtitle(&mut self, id: i32) -> Result<(), String> {
        let Some(set_track) = self.api.video_set_spu else {
            return Err("This VLC runtime does not expose subtitle track selection.".to_owned());
        };
        if unsafe { set_track(self.player, id) } != 0 {
            return Err(format!(
                "Could not select the subtitle: {}",
                self.last_error()
            ));
        }
        Ok(())
    }

    fn set_equalizer(
        &mut self,
        enabled: bool,
        preamp: f32,
        bands: [f32; 10],
    ) -> Result<(), String> {
        self.equalizer_settings = (enabled, preamp, bands);
        let Some(create) = self.api.equalizer_new else {
            return Err("This VLC runtime does not support its equalizer API.".to_owned());
        };
        let Some(set_equalizer) = self.api.player_set_equalizer else {
            return Err("This VLC runtime does not support its equalizer API.".to_owned());
        };
        if self.equalizer.is_null() {
            self.equalizer = unsafe { create() };
            if self.equalizer.is_null() {
                return Err("libVLC could not create an equalizer.".to_owned());
            }
        }
        if let Some(set_preamp) = self.api.equalizer_set_preamp {
            if unsafe { set_preamp(self.equalizer, preamp) } != 0 {
                return Err("libVLC could not apply the equalizer preamp.".to_owned());
            }
        }
        if let Some(set_band) = self.api.equalizer_set_band {
            for (index, value) in bands.iter().enumerate() {
                if unsafe { set_band(self.equalizer, *value, index as c_uint) } != 0 {
                    return Err(format!(
                        "libVLC could not apply equalizer band {}.",
                        index + 1
                    ));
                }
            }
        }
        let target = if enabled {
            self.equalizer
        } else {
            ptr::null_mut()
        };
        if unsafe { set_equalizer(self.player, target) } != 0 {
            return Err(format!(
                "libVLC could not apply the equalizer: {}",
                self.last_error()
            ));
        }
        Ok(())
    }

    fn apply_audio_settings(&mut self) -> Result<(), String> {
        self.set_volume(self.volume)?;
        self.set_muted(self.muted)?;
        if self.rate != 1.0 {
            self.set_rate(self.rate)?;
        }
        let (enabled, preamp, bands) = self.equalizer_settings;
        if enabled {
            self.set_equalizer(enabled, preamp, bands)?;
        }
        Ok(())
    }

    fn schedule_probes(
        &mut self,
        paths: Vec<PathBuf>,
        probes: &mut Vec<Probe>,
        snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    ) {
        for path in paths {
            let Ok(path_text) = path.to_str().ok_or(()) else {
                continue;
            };
            let Ok(path_c) = CString::new(path_text) else {
                continue;
            };
            let media = unsafe { (self.api.media_new_path)(self.instance, path_c.as_ptr()) };
            if media.is_null() {
                continue;
            }
            unsafe { (self.api.media_parse_async)(media) };
            probes.push(Probe {
                path,
                media,
                deadline: Instant::now() + Duration::from_secs(30),
            });
        }
        if !probes.is_empty() {
            update_snapshot(snapshot, |state| state.error = None);
        }
    }

    fn refresh(&mut self, snapshot: &Arc<Mutex<PlaybackSnapshot>>) {
        if self.media.is_null() {
            return;
        }
        let time = unsafe { (self.api.player_get_time)(self.player) };
        let length = unsafe { (self.api.player_get_length)(self.player) };
        let position = (time >= 0).then(|| Duration::from_millis(time as u64));
        let duration = (length > 0).then(|| Duration::from_millis(length as u64));
        match unsafe { (self.api.player_get_state)(self.player) } {
            1 | 2 => self.state = PlaybackState::Loading,
            3 => self.state = PlaybackState::Playing,
            4 => self.state = PlaybackState::Paused,
            5 | 6 => self.state = PlaybackState::Stopped,
            7 => {
                self.state = PlaybackState::Stopped;
                let error = self.last_error();
                update_snapshot(snapshot, |snapshot| snapshot.error = Some(error));
            }
            _ => {}
        }
        let state = self.state;
        update_snapshot(snapshot, |snapshot| {
            snapshot.ready = true;
            snapshot.path = self.current_path.clone();
            snapshot.position = position;
            snapshot.duration = duration;
            snapshot.state = state;
            snapshot.volume = self.volume;
            snapshot.muted = self.muted;
            snapshot.rate = self.rate;
        });

        if self.last_track_poll.elapsed() >= Duration::from_secs(1) {
            self.last_track_poll = Instant::now();
            let audio_tracks = self.read_track_list(
                self.api.audio_get_track_description,
                self.api.track_description_release,
            );
            let subtitles = self.read_track_list(
                self.api.video_get_spu_description,
                self.api.track_description_release,
            );
            update_snapshot(snapshot, |snapshot| {
                snapshot.audio_tracks = audio_tracks;
                snapshot.subtitles = subtitles;
            });
        }
    }

    fn read_track_list(
        &self,
        list_fn: Option<unsafe extern "C" fn(*mut c_void) -> *mut TrackDescription>,
        release_fn: Option<unsafe extern "C" fn(*mut TrackDescription)>,
    ) -> Vec<TrackOption> {
        let (Some(list_fn), Some(release_fn)) = (list_fn, release_fn) else {
            return Vec::new();
        };
        let head = unsafe { list_fn(self.player) };
        let mut options = Vec::new();
        let mut current = head;
        while !current.is_null() {
            let item = unsafe { &*current };
            let label = if item.name.is_null() {
                format!("Track {}", item.id)
            } else {
                unsafe { CStr::from_ptr(item.name) }
                    .to_string_lossy()
                    .into_owned()
            };
            options.push(TrackOption { id: item.id, label });
            current = item.next;
        }
        if !head.is_null() {
            unsafe { release_fn(head) };
        }
        options
    }

    fn last_error(&self) -> String {
        let error = unsafe { (self.api.error_message)() };
        if error.is_null() {
            "unknown libVLC error".to_owned()
        } else {
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        }
    }
}

impl Drop for VlcEngine {
    fn drop(&mut self) {
        self.stop_current();
        if !self.equalizer.is_null() {
            if let Some(release) = self.api.equalizer_release {
                unsafe { release(self.equalizer) };
            }
        }
        unsafe {
            (self.api.player_release)(self.player);
            (self.api.instance_release)(self.instance);
        }
    }
}

fn poll_probes(
    engine: &mut VlcEngine,
    snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    probes: &mut Vec<Probe>,
) {
    let mut index = 0;
    while index < probes.len() {
        let probe = &probes[index];
        let parsed = unsafe { (engine.api.media_is_parsed)(probe.media) } != 0;
        if parsed || Instant::now() >= probe.deadline {
            let probe = probes.swap_remove(index);
            if parsed {
                let mut metadata = TrackMetadata {
                    duration: match unsafe { (engine.api.media_get_duration)(probe.media) } {
                        duration if duration >= 0 => Some(Duration::from_millis(duration as u64)),
                        _ => None,
                    },
                    ..TrackMetadata::default()
                };
                metadata.title = engine.read_meta(probe.media, 0);
                metadata.artist = engine.read_meta(probe.media, 1);
                metadata.album = engine.read_meta(probe.media, 4);
                update_snapshot(snapshot, |state| {
                    state.metadata.insert(probe.path.clone(), metadata);
                });
            }
            unsafe { (engine.api.media_release)(probe.media) };
        } else {
            index += 1;
        }
    }
}

impl VlcEngine {
    fn read_meta(&self, media: *mut c_void, key: c_int) -> Option<String> {
        let value_ptr = unsafe { (self.api.media_get_meta)(media, key) };
        if value_ptr.is_null() {
            return None;
        }
        let value = unsafe { CStr::from_ptr(value_ptr) }
            .to_string_lossy()
            .trim()
            .to_owned();
        unsafe { (self.api.free)(value_ptr as *mut c_void) };
        (!value.is_empty()).then_some(value)
    }
}

fn append_history(path: &Path) -> Result<(), String> {
    let Some(app_data) = std::env::var_os("APPDATA") else {
        return Err("APPDATA is not set; playback history could not be saved.".to_owned());
    };
    let directory = PathBuf::from(app_data).join("RetroWave Player");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        return Err(format!(
            "Could not create playback history directory: {error}"
        ));
    }
    let history = directory.join("history.log");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history)
        .and_then(|mut file| {
            use std::io::Write;
            writeln!(file, "{}", path.display())
        })
        .map_err(|error| format!("Could not write playback history: {error}"))
}

fn load_vlc_library() -> Result<(Library, PathBuf, Option<PathBuf>), String> {
    let candidates = vlc_library_candidates();
    let configured_plugins = std::env::var_os("VLC_PLUGIN_PATH").map(PathBuf::from);
    if let Some(plugin_path) = &configured_plugins {
        if !plugin_path.is_dir() {
            return Err(format!(
                "VLC_PLUGIN_PATH does not point to a plugin directory: {}",
                plugin_path.display()
            ));
        }
    }
    for path in candidates {
        if !path.is_file() {
            continue;
        }
        let library = load_library(&path)?;
        let plugin_path = configured_plugins
            .clone()
            .or_else(|| path.parent().map(|parent| parent.join("plugins")))
            .filter(|plugin_path| plugin_path.is_dir());
        if plugin_path.is_none() {
            return Err(format!(
                "libVLC was found at {}, but its plugins directory was not found. Set VLC_PLUGIN_PATH to the matching VLC plugins folder.",
                path.display()
            ));
        }

        return Ok((library, path, plugin_path));
    }
    Err("libVLC was not found. Install the matching x64 VLC desktop runtime or set LIBVLC_PATH to libvlc.dll.".to_owned())
}

fn vlc_library_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(configured) = std::env::var_os("LIBVLC_PATH") {
        let configured = PathBuf::from(configured);
        candidates.push(if configured.is_dir() {
            configured.join("libvlc.dll")
        } else {
            configured
        });
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("libvlc.dll"));
        }
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(program_files) = std::env::var_os(variable) {
            candidates.push(
                PathBuf::from(program_files)
                    .join("VideoLAN")
                    .join("VLC")
                    .join("libvlc.dll"),
            );
        }
    }
    candidates
}

#[cfg(windows)]
fn load_library(path: &Path) -> Result<Library, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::GetLastError;
    use windows::Win32::System::LibraryLoader::{
        LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LoadLibraryExW,
    };
    use windows::core::PCWSTR;

    let path_wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
    let module = unsafe { LoadLibraryExW(PCWSTR(path_wide.as_ptr()), None, flags) };
    let module = match module {
        Ok(module) => module,
        Err(error) => {
            let error_code = unsafe { GetLastError().0 };
            return Err(format!(
                "LoadLibraryExW failed for {} with Win32 error {error_code} (0x{error_code:08X}): {error}",
                path.display()
            ));
        }
    };
    let library = unsafe { libloading::os::windows::Library::from_raw(module.0 as isize) };
    Ok(library.into())
}

#[cfg(not(windows))]
fn load_library(path: &Path) -> Result<Library, String> {
    unsafe { Library::new(path) }
        .map_err(|error| format!("Could not load {}: {error}", path.display()))
}

#[repr(C)]
struct TrackDescription {
    id: c_int,
    name: *mut c_char,
    next: *mut TrackDescription,
}

struct VlcApi {
    _library: Library,
    instance_new: unsafe extern "C" fn(c_int, *const *const c_char) -> *mut c_void,
    instance_release: unsafe extern "C" fn(*mut c_void),
    error_message: unsafe extern "C" fn() -> *const c_char,
    free: unsafe extern "C" fn(*mut c_void),
    media_new_path: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    media_release: unsafe extern "C" fn(*mut c_void),
    media_parse_async: unsafe extern "C" fn(*mut c_void),
    media_is_parsed: unsafe extern "C" fn(*mut c_void) -> c_int,
    media_get_duration: unsafe extern "C" fn(*mut c_void) -> i64,
    media_get_meta: unsafe extern "C" fn(*mut c_void, c_int) -> *mut c_char,
    player_new: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    player_release: unsafe extern "C" fn(*mut c_void),
    player_set_media: unsafe extern "C" fn(*mut c_void, *mut c_void),
    player_play: unsafe extern "C" fn(*mut c_void) -> c_int,
    player_get_state: unsafe extern "C" fn(*mut c_void) -> c_int,
    player_set_pause: unsafe extern "C" fn(*mut c_void, c_int),
    player_stop: unsafe extern "C" fn(*mut c_void),
    player_get_time: unsafe extern "C" fn(*mut c_void) -> i64,
    player_get_length: unsafe extern "C" fn(*mut c_void) -> i64,
    player_set_time: unsafe extern "C" fn(*mut c_void, i64),
    player_set_hwnd: unsafe extern "C" fn(*mut c_void, *mut c_void),
    audio_set_volume: unsafe extern "C" fn(*mut c_void, c_int) -> c_int,
    audio_set_mute: unsafe extern "C" fn(*mut c_void, c_int),
    player_set_rate: unsafe extern "C" fn(*mut c_void, c_float) -> c_int,
    player_add_slave:
        Option<unsafe extern "C" fn(*mut c_void, c_int, *const c_char, c_int) -> c_int>,
    audio_set_track: Option<unsafe extern "C" fn(*mut c_void, c_int) -> c_int>,
    audio_get_track_description: Option<unsafe extern "C" fn(*mut c_void) -> *mut TrackDescription>,
    video_set_spu: Option<unsafe extern "C" fn(*mut c_void, c_int) -> c_int>,
    video_get_spu_description: Option<unsafe extern "C" fn(*mut c_void) -> *mut TrackDescription>,
    track_description_release: Option<unsafe extern "C" fn(*mut TrackDescription)>,
    equalizer_new: Option<unsafe extern "C" fn() -> *mut c_void>,
    equalizer_release: Option<unsafe extern "C" fn(*mut c_void)>,
    equalizer_set_preamp: Option<unsafe extern "C" fn(*mut c_void, c_float) -> c_int>,
    equalizer_set_band: Option<unsafe extern "C" fn(*mut c_void, c_float, c_uint) -> c_int>,
    player_set_equalizer: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int>,
}

impl VlcApi {
    unsafe fn load(library: Library) -> Result<Self, String> {
        unsafe fn required<T: Copy>(library: &Library, name: &[u8]) -> Result<T, String> {
            // The pointer is copied only while the owning Library remains stored in VlcApi.
            unsafe { library.get::<T>(name) }
                .map(|symbol| *symbol)
                .map_err(|error| {
                    format!(
                        "The installed VLC runtime is missing {}: {error}",
                        String::from_utf8_lossy(&name[..name.len() - 1])
                    )
                })
        }
        unsafe fn optional<T: Copy>(library: &Library, name: &[u8]) -> Option<T> {
            unsafe { library.get::<T>(name) }.ok().map(|symbol| *symbol)
        }

        Ok(Self {
            instance_new: unsafe { required(&library, b"libvlc_new\0")? },
            instance_release: unsafe { required(&library, b"libvlc_release\0")? },
            error_message: unsafe { required(&library, b"libvlc_errmsg\0")? },
            free: unsafe { required(&library, b"libvlc_free\0")? },
            media_new_path: unsafe { required(&library, b"libvlc_media_new_path\0")? },
            media_release: unsafe { required(&library, b"libvlc_media_release\0")? },
            media_parse_async: unsafe { required(&library, b"libvlc_media_parse_async\0")? },
            media_is_parsed: unsafe { required(&library, b"libvlc_media_is_parsed\0")? },
            media_get_duration: unsafe { required(&library, b"libvlc_media_get_duration\0")? },
            media_get_meta: unsafe { required(&library, b"libvlc_media_get_meta\0")? },
            player_new: unsafe { required(&library, b"libvlc_media_player_new\0")? },
            player_release: unsafe { required(&library, b"libvlc_media_player_release\0")? },
            player_set_media: unsafe { required(&library, b"libvlc_media_player_set_media\0")? },
            player_play: unsafe { required(&library, b"libvlc_media_player_play\0")? },
            player_get_state: unsafe { required(&library, b"libvlc_media_player_get_state\0")? },
            player_set_pause: unsafe { required(&library, b"libvlc_media_player_set_pause\0")? },
            player_stop: unsafe { required(&library, b"libvlc_media_player_stop\0")? },
            player_get_time: unsafe { required(&library, b"libvlc_media_player_get_time\0")? },
            player_get_length: unsafe { required(&library, b"libvlc_media_player_get_length\0")? },
            player_set_time: unsafe { required(&library, b"libvlc_media_player_set_time\0")? },
            player_set_hwnd: unsafe { required(&library, b"libvlc_media_player_set_hwnd\0")? },
            audio_set_volume: unsafe { required(&library, b"libvlc_audio_set_volume\0")? },
            audio_set_mute: unsafe { required(&library, b"libvlc_audio_set_mute\0")? },
            player_set_rate: unsafe { required(&library, b"libvlc_media_player_set_rate\0")? },
            player_add_slave: unsafe { optional(&library, b"libvlc_media_player_add_slave\0") },
            audio_set_track: unsafe { optional(&library, b"libvlc_audio_set_track\0") },
            audio_get_track_description: unsafe {
                optional(&library, b"libvlc_audio_get_track_description\0")
            },
            video_set_spu: unsafe { optional(&library, b"libvlc_video_set_spu\0") },
            video_get_spu_description: unsafe {
                optional(&library, b"libvlc_video_get_spu_description\0")
            },
            track_description_release: unsafe {
                optional(&library, b"libvlc_track_description_list_release\0")
            },
            equalizer_new: unsafe { optional(&library, b"libvlc_audio_equalizer_new\0") },
            equalizer_release: unsafe { optional(&library, b"libvlc_audio_equalizer_release\0") },
            equalizer_set_preamp: unsafe {
                optional(&library, b"libvlc_audio_equalizer_set_preamp\0")
            },
            equalizer_set_band: unsafe {
                optional(&library, b"libvlc_audio_equalizer_set_amp_at_index\0")
            },
            player_set_equalizer: unsafe {
                optional(&library, b"libvlc_media_player_set_equalizer\0")
            },
            _library: library,
        })
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::load_library;

    #[test]
    fn windows_load_failure_reports_the_original_error_code() {
        let missing = std::env::temp_dir().join("retrowave-missing-libvlc-test.dll");
        let error = match load_library(&missing) {
            Ok(_) => panic!("a missing DLL must not load"),
            Err(error) => error,
        };
        assert!(error.contains("LoadLibraryExW failed"));
        assert!(error.contains("(0x"));
        let code = error
            .split("Win32 error ")
            .nth(1)
            .and_then(|remaining| remaining.split_whitespace().next())
            .and_then(|code| code.parse::<u32>().ok())
            .expect("diagnostic must include the numeric Win32 error code");
        assert_ne!(code, 0);
    }
}

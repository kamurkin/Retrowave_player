use libloading::Library;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_float, c_int, c_uint, c_void};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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

fn is_no_audio_output_error(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("no active audio output")
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DeferredPauseState {
    #[default]
    None,
    Requested,
    PauseSent,
    ResumeSent,
}

fn toggle_loading_pause_request(state: DeferredPauseState) -> DeferredPauseState {
    match state {
        DeferredPauseState::None => DeferredPauseState::Requested,
        DeferredPauseState::Requested => DeferredPauseState::None,
        DeferredPauseState::PauseSent => DeferredPauseState::ResumeSent,
        DeferredPauseState::ResumeSent => DeferredPauseState::PauseSent,
    }
}

fn apply_confirmed_vlc_state(
    phase: PlayRequestPhase,
    deferred: DeferredPauseState,
    vlc_state: i32,
) -> (PlayRequestPhase, DeferredPauseState, PlaybackState, bool) {
    match vlc_state {
        3 => match deferred {
            DeferredPauseState::Requested => (
                phase,
                DeferredPauseState::PauseSent,
                PlaybackState::Loading,
                true,
            ),
            DeferredPauseState::PauseSent => (
                phase,
                DeferredPauseState::PauseSent,
                PlaybackState::Loading,
                false,
            ),
            DeferredPauseState::ResumeSent => {
                let phase = match phase {
                    PlayRequestPhase::Loading(id) | PlayRequestPhase::PendingDelay(id) => {
                        PlayRequestPhase::Active(id)
                    }
                    other => other,
                };
                (
                    phase,
                    DeferredPauseState::None,
                    PlaybackState::Playing,
                    false,
                )
            }
            DeferredPauseState::None => {
                let phase = match phase {
                    PlayRequestPhase::Loading(id) | PlayRequestPhase::PendingDelay(id) => {
                        PlayRequestPhase::Active(id)
                    }
                    other => other,
                };
                (
                    phase,
                    DeferredPauseState::None,
                    PlaybackState::Playing,
                    false,
                )
            }
        },
        4 => {
            let phase = match phase {
                PlayRequestPhase::Loading(id) | PlayRequestPhase::PendingDelay(id) => {
                    PlayRequestPhase::Active(id)
                }
                other => other,
            };
            (
                phase,
                DeferredPauseState::None,
                PlaybackState::Paused,
                false,
            )
        }
        1 | 2 => (phase, deferred, PlaybackState::Loading, false),
        5 | 6 | 7 => (
            PlayRequestPhase::None,
            DeferredPauseState::None,
            PlaybackState::Stopped,
            false,
        ),
        _ => (phase, deferred, PlaybackState::Loading, false),
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlayRequestPhase {
    #[default]
    None,
    PendingDelay(u64),
    Loading(u64),
    Active(u64),
}

#[cfg(debug_assertions)]
const DEFAULT_DEBUG_LOADING_DELAY_MS: u64 = 3000;

#[cfg(debug_assertions)]
fn debug_loading_delay_ms() -> u64 {
    std::env::var("RETROWAVE_DEBUG_LOADING_DELAY_MS")
        .ok()
        .as_deref()
        .map(parse_debug_loading_delay_ms)
        .unwrap_or(DEFAULT_DEBUG_LOADING_DELAY_MS)
}

#[cfg(debug_assertions)]
fn parse_debug_loading_delay_ms(value: &str) -> u64 {
    value
        .parse::<u64>()
        .ok()
        .unwrap_or(DEFAULT_DEBUG_LOADING_DELAY_MS)
}

#[cfg(debug_assertions)]
#[derive(Clone)]
struct PendingPlay {
    command: Command,
    ready_at: Instant,
}

#[cfg(debug_assertions)]
fn pending_play_is_ready(ready_at: Instant, now: Instant) -> bool {
    now >= ready_at
}

#[cfg(debug_assertions)]
impl PendingPlay {
    fn new(command: Command, delay_ms: u64, now: Instant) -> Self {
        Self {
            command,
            ready_at: now + Duration::from_millis(delay_ms),
        }
    }

    fn is_ready(&self, now: Instant) -> bool {
        pending_play_is_ready(self.ready_at, now)
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
    pub play_request_phase: PlayRequestPhase,
    pub natural_end_event: Option<NaturalEndEvent>,
    pub path: Option<PathBuf>,
    pub position: Option<Duration>,
    pub duration: Option<Duration>,
    pub volume: u8,
    pub muted: bool,
    pub rate: f32,
    pub error: Option<String>,
    pub history_warning: Option<String>,
    pub video_play_result: Option<VideoPlayResult>,
    pub metadata: HashMap<PathBuf, TrackMetadata>,
    pub audio_tracks: Vec<TrackOption>,
    pub subtitles: Vec<TrackOption>,
    pub equalizer_supported: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VideoPlayResult {
    pub request_id: u64,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NaturalEndEvent {
    pub event_id: u64,
    pub generation: u64,
    pub request_id: u64,
}

impl Default for PlaybackSnapshot {
    fn default() -> Self {
        Self {
            ready: false,
            state: PlaybackState::Stopped,
            play_request_phase: PlayRequestPhase::None,
            natural_end_event: None,
            path: None,
            position: None,
            duration: None,
            volume: 70,
            muted: false,
            rate: 1.0,
            error: None,
            history_warning: None,
            video_play_result: None,
            metadata: HashMap::new(),
            audio_tracks: Vec::new(),
            subtitles: Vec::new(),
            equalizer_supported: false,
        }
    }
}

#[derive(Clone)]
enum Command {
    Play {
        path: PathBuf,
        target: usize,
        request_id: u64,
        is_video: bool,
    },
    Pause {
        request_id: Option<u64>,
    },
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
    NaturalEndReached {
        generation: u64,
        request_id: u64,
    },
    AcknowledgeNaturalEnd {
        event_id: u64,
        generation: u64,
        request_id: u64,
    },
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
        let callback_sender = sender.clone();
        if let Err(error) = thread::Builder::new()
            .name("retrowave-libvlc".to_owned())
            .spawn(move || playback_worker(receiver, worker_snapshot, callback_sender))
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

    pub fn play(&self, path: PathBuf, video_target: usize, request_id: u64, is_video: bool) {
        self.send(Command::Play {
            path,
            target: video_target,
            request_id,
            is_video,
        });
    }

    pub fn pause(&self) {
        self.send(Command::Pause { request_id: None });
    }

    pub fn pause_for_request(&self, request_id: u64) {
        self.send(Command::Pause {
            request_id: Some(request_id),
        });
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    pub fn acknowledge_natural_end(&self, event: &NaturalEndEvent) {
        self.send(Command::AcknowledgeNaturalEnd {
            event_id: event.event_id,
            generation: event.generation,
            request_id: event.request_id,
        });
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

fn playback_worker(
    receiver: Receiver<Command>,
    snapshot: Arc<Mutex<PlaybackSnapshot>>,
    callback_sender: Sender<Command>,
) {
    let mut engine = match VlcEngine::new(callback_sender) {
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
    #[cfg(debug_assertions)]
    let mut pending_play: Option<PendingPlay> = None;
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => {
                #[cfg(debug_assertions)]
                {
                    let delayed_request_id = match &command {
                        Command::Play { request_id, .. } => Some(*request_id),
                        _ => None,
                    };
                    if let Some(request_id) = delayed_request_id {
                        let delay_ms = debug_loading_delay_ms();
                        if delay_ms > 0 {
                            engine.state = PlaybackState::Loading;
                            engine.play_request_phase = PlayRequestPhase::PendingDelay(request_id);
                            engine.deferred_pause = DeferredPauseState::None;
                            pending_play =
                                Some(PendingPlay::new(command, delay_ms, Instant::now()));
                            update_snapshot(&snapshot, |state| {
                                state.state = PlaybackState::Loading;
                                state.play_request_phase =
                                    PlayRequestPhase::PendingDelay(request_id);
                                state.error = None;
                                state.video_play_result = None;
                            });
                            continue;
                        }
                        pending_play = None;
                    }
                    if matches!(command, Command::Stop) {
                        pending_play = None;
                    }
                }
                match process_command(command, &mut engine, &snapshot, &mut probes, false) {
                    true => break,
                    false => {}
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        #[cfg(debug_assertions)]
        if pending_play
            .as_ref()
            .is_some_and(|pending| pending.is_ready(Instant::now()))
        {
            let command = pending_play
                .take()
                .expect("pending Play was checked")
                .command;
            let _ = process_command(command, &mut engine, &snapshot, &mut probes, true);
        }
        #[cfg(debug_assertions)]
        if pending_play.is_none() {
            engine.refresh(&snapshot);
        }
        #[cfg(not(debug_assertions))]
        engine.refresh(&snapshot);
        poll_probes(&mut engine, &snapshot, &mut probes);
    }
}

fn process_command(
    command: Command,
    engine: &mut VlcEngine,
    snapshot: &Arc<Mutex<PlaybackSnapshot>>,
    probes: &mut Vec<Probe>,
    preserve_pending_pause: bool,
) -> bool {
    let play_result = match &command {
        Command::Play {
            request_id,
            is_video,
            ..
        } => Some((*request_id, *is_video)),
        _ => None,
    };
    let stop_result = matches!(&command, Command::Stop);
    let result = match command {
        Command::Play {
            path,
            target,
            request_id,
            is_video: _,
        } => {
            if !preserve_pending_pause {
                engine.deferred_pause = DeferredPauseState::None;
            }
            engine.play(path, target, request_id)
        }
        Command::Pause { request_id } => engine.pause(request_id),
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
        Command::NaturalEndReached {
            generation,
            request_id,
        } => {
            if natural_end_matches_active_request(
                engine.active_generation,
                engine.active_request_id,
                generation,
                request_id,
            ) && engine.natural_end_pending.is_none()
            {
                engine.next_natural_end_event_id = engine.next_natural_end_event_id.wrapping_add(1);
                if engine.next_natural_end_event_id == 0 {
                    engine.next_natural_end_event_id = 1;
                }
                let event = NaturalEndEvent {
                    event_id: engine.next_natural_end_event_id,
                    generation,
                    request_id,
                };
                engine.natural_end_pending = Some(event.clone());
                engine.invalidate_end_callback();
                update_snapshot(snapshot, |state| {
                    state.natural_end_event = Some(event);
                });
            }
            Ok(())
        }
        Command::AcknowledgeNaturalEnd {
            event_id,
            generation,
            request_id,
        } => {
            if engine.natural_end_pending.as_ref().is_some_and(|event| {
                event.event_id == event_id
                    && event.generation == generation
                    && event.request_id == request_id
            }) {
                engine.natural_end_pending = None;
                update_snapshot(snapshot, |state| state.natural_end_event = None);
            }
            Ok(())
        }
    };
    match result {
        Ok(()) => update_snapshot(snapshot, |state| {
            state.error = None;
            state.volume = engine.volume;
            state.muted = engine.muted;
            state.rate = engine.rate;
            state.play_request_phase = engine.play_request_phase;
            state.state = engine.state;
            if play_result.is_some() || stop_result {
                state.natural_end_event = None;
            }
            if let Some((request_id, is_video)) = play_result {
                state.history_warning = engine.history_warning.clone();
                state.video_play_result = if is_video {
                    engine
                        .current_path
                        .clone()
                        .map(|path| VideoPlayResult { request_id, path })
                } else {
                    None
                };
            }
        }),
        Err(error) => {
            if play_result.is_some() {
                engine.deferred_pause = DeferredPauseState::None;
                engine.play_request_phase = PlayRequestPhase::None;
                engine.active_generation = 0;
                engine.active_request_id = None;
                engine.natural_end_pending = None;
                engine.invalidate_end_callback();
            }
            update_snapshot(snapshot, |state| {
                state.error = Some(error);
                state.play_request_phase = engine.play_request_phase;
                if play_result.is_some() {
                    state.history_warning = engine.history_warning.clone();
                    state.natural_end_event = None;
                }
                if let Some((request_id, _)) = play_result {
                    let _ = request_id;
                    state.video_play_result = None;
                }
            });
        }
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

const VLC_EVENT_MEDIA_PLAYER_END_REACHED: c_int = 265;

struct EndCallbackContext {
    sender: Sender<Command>,
    active: AtomicBool,
    callbacks_in_flight: AtomicUsize,
    generation: AtomicU64,
    request_id: AtomicU64,
}

fn natural_end_matches_active_request(
    active_generation: u64,
    active_request_id: Option<u64>,
    generation: u64,
    request_id: u64,
) -> bool {
    active_generation != 0
        && active_generation == generation
        && active_request_id == Some(request_id)
}

impl EndCallbackContext {
    fn activate(&self, generation: u64, request_id: u64) {
        while self.callbacks_in_flight.load(Ordering::Acquire) != 0 {
            thread::yield_now();
        }
        self.generation.store(generation, Ordering::Release);
        self.request_id.store(request_id, Ordering::Release);
        self.active.store(true, Ordering::Release);
    }

    fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        while self.callbacks_in_flight.load(Ordering::Acquire) != 0 {
            thread::yield_now();
        }
    }
}

extern "C" fn vlc_end_reached_callback(_event: *const c_void, user_data: *mut c_void) {
    if user_data.is_null() {
        return;
    }
    let context = unsafe { &*(user_data.cast::<EndCallbackContext>()) };
    context.callbacks_in_flight.fetch_add(1, Ordering::AcqRel);
    if context.active.load(Ordering::Acquire) {
        let generation = context.generation.load(Ordering::Acquire);
        let request_id = context.request_id.load(Ordering::Acquire);
        let _ = context.sender.send(Command::NaturalEndReached {
            generation,
            request_id,
        });
    }
    context.callbacks_in_flight.fetch_sub(1, Ordering::AcqRel);
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
    volume_pending: bool,
    state: PlaybackState,
    play_request_phase: PlayRequestPhase,
    current_path: Option<PathBuf>,
    history_warning: Option<String>,
    deferred_pause: DeferredPauseState,
    equalizer_settings: (bool, f32, [f32; 10]),
    last_track_poll: Instant,
    callback_context: Box<EndCallbackContext>,
    callback_manager: *mut c_void,
    callback_attached: bool,
    active_generation: u64,
    active_request_id: Option<u64>,
    next_generation: u64,
    next_natural_end_event_id: u64,
    natural_end_pending: Option<NaturalEndEvent>,
}

impl VlcEngine {
    fn new(callback_sender: Sender<Command>) -> Result<Self, String> {
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
            volume_pending: false,
            state: PlaybackState::Stopped,
            play_request_phase: PlayRequestPhase::None,
            current_path: None,
            history_warning: None,
            deferred_pause: DeferredPauseState::None,
            equalizer_settings: (false, 0.0, [0.0; 10]),
            last_track_poll: Instant::now(),
            callback_context: Box::new(EndCallbackContext {
                sender: callback_sender,
                active: AtomicBool::new(false),
                callbacks_in_flight: AtomicUsize::new(0),
                generation: AtomicU64::new(0),
                request_id: AtomicU64::new(0),
            }),
            callback_manager: ptr::null_mut(),
            callback_attached: false,
            active_generation: 0,
            active_request_id: None,
            next_generation: 0,
            next_natural_end_event_id: 0,
            natural_end_pending: None,
        };
        engine.set_volume(70)?;
        Ok(engine)
    }

    fn invalidate_end_callback(&mut self) {
        self.callback_context.deactivate();
        if self.callback_attached && !self.callback_manager.is_null() {
            unsafe {
                (self.api.event_detach)(
                    self.callback_manager,
                    VLC_EVENT_MEDIA_PLAYER_END_REACHED,
                    vlc_end_reached_callback,
                    self.callback_context.as_ref() as *const EndCallbackContext as *mut c_void,
                );
            }
        }
        self.callback_context.deactivate();
        self.callback_attached = false;
        self.callback_manager = ptr::null_mut();
    }

    fn attach_end_callback(&mut self, generation: u64, request_id: u64) -> Result<(), String> {
        self.invalidate_end_callback();
        let manager = unsafe { (self.api.player_event_manager)(self.player) };
        if manager.is_null() {
            return Err("libVLC could not provide the media player event manager.".to_owned());
        }
        self.callback_manager = manager;
        self.callback_context.activate(generation, request_id);
        let result = unsafe {
            (self.api.event_attach)(
                manager,
                VLC_EVENT_MEDIA_PLAYER_END_REACHED,
                vlc_end_reached_callback,
                self.callback_context.as_ref() as *const EndCallbackContext as *mut c_void,
            )
        };
        if result != 0 {
            self.invalidate_end_callback();
            return Err("libVLC could not attach the natural end callback.".to_owned());
        }
        self.callback_attached = true;
        Ok(())
    }

    fn play(&mut self, path: PathBuf, video_target: usize, request_id: u64) -> Result<(), String> {
        self.history_warning = None;
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
        self.next_generation = self.next_generation.wrapping_add(1);
        if self.next_generation == 0 {
            self.next_generation = 1;
        }
        let generation = self.next_generation;
        unsafe {
            (self.api.player_set_media)(self.player, media);
            (self.api.media_parse_async)(media);
        }
        self.media = media;
        self.video_target = video_target;
        unsafe { (self.api.player_set_hwnd)(self.player, video_target as *mut c_void) };
        self.active_generation = generation;
        self.active_request_id = Some(request_id);
        self.natural_end_pending = None;
        if let Err(error) = self.attach_end_callback(generation, request_id) {
            self.active_generation = 0;
            self.active_request_id = None;
            self.stop_current();
            return Err(error);
        }
        if (unsafe { (self.api.player_play)(self.player) }) != 0 {
            self.stop_current();
            return Err(format!(
                "libVLC failed to start {}: {}",
                path.display(),
                self.last_error()
            ));
        }
        self.state = PlaybackState::Loading;
        self.play_request_phase = PlayRequestPhase::Loading(request_id);
        self.current_path = Some(path.clone());
        self.apply_audio_settings()?;
        self.history_warning = append_history(&path).err();
        Ok(())
    }

    fn pause(&mut self, request_id: Option<u64>) -> Result<(), String> {
        match self.play_request_phase {
            PlayRequestPhase::PendingDelay(active_id) | PlayRequestPhase::Loading(active_id) => {
                if request_id.is_some_and(|id| id != active_id) {
                    return Ok(());
                }
                let previous = self.deferred_pause;
                let next = toggle_loading_pause_request(previous);
                match (previous, next) {
                    (DeferredPauseState::PauseSent, DeferredPauseState::ResumeSent) => {
                        unsafe { (self.api.player_set_pause)(self.player, 0) };
                    }
                    (DeferredPauseState::ResumeSent, DeferredPauseState::PauseSent) => {
                        unsafe { (self.api.player_set_pause)(self.player, 1) };
                    }
                    _ => {}
                }
                self.deferred_pause = next;
                return Ok(());
            }
            PlayRequestPhase::Active(active_id) => {
                if request_id.is_some_and(|id| id != active_id) {
                    return Ok(());
                }
            }
            PlayRequestPhase::None => {}
        }
        if self.media.is_null() {
            return Err("Load a track before using Pause.".to_owned());
        }
        self.deferred_pause = match self.deferred_pause {
            DeferredPauseState::PauseSent => {
                unsafe { (self.api.player_set_pause)(self.player, 0) };
                DeferredPauseState::ResumeSent
            }
            DeferredPauseState::ResumeSent => {
                unsafe { (self.api.player_set_pause)(self.player, 1) };
                DeferredPauseState::PauseSent
            }
            DeferredPauseState::None | DeferredPauseState::Requested => {
                let next = if self.state == PlaybackState::Playing {
                    PlaybackState::Paused
                } else {
                    PlaybackState::Playing
                };
                unsafe {
                    (self.api.player_set_pause)(
                        self.player,
                        i32::from(next == PlaybackState::Paused),
                    )
                };
                if next == PlaybackState::Paused {
                    DeferredPauseState::PauseSent
                } else {
                    DeferredPauseState::ResumeSent
                }
            }
        };
        Ok(())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.deferred_pause = DeferredPauseState::None;
        self.play_request_phase = PlayRequestPhase::None;
        self.active_generation = 0;
        self.active_request_id = None;
        self.natural_end_pending = None;
        self.invalidate_end_callback();
        if !self.media.is_null() {
            unsafe { (self.api.player_stop)(self.player) };
        }
        self.state = PlaybackState::Stopped;
        Ok(())
    }

    fn stop_current(&mut self) {
        self.invalidate_end_callback();
        self.active_generation = 0;
        self.active_request_id = None;
        self.natural_end_pending = None;
        if !self.media.is_null() {
            unsafe {
                (self.api.player_stop)(self.player);
                (self.api.player_set_media)(self.player, ptr::null_mut());
                (self.api.media_release)(self.media);
            }
            self.media = ptr::null_mut();
        }
        self.current_path = None;
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
            let error = self.last_error();
            if is_no_audio_output_error(&error) {
                self.volume = volume;
                self.volume_pending = true;
                return Ok(());
            }
            return Err(format!("libVLC could not set volume: {error}"));
        }
        self.volume = volume;
        self.volume_pending = false;
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
            let state = self.state;
            update_snapshot(snapshot, |snapshot| {
                snapshot.path = self.current_path.clone();
                snapshot.position = None;
                snapshot.duration = None;
                snapshot.state = state;
                snapshot.play_request_phase = self.play_request_phase;
            });
            return;
        }
        let time = unsafe { (self.api.player_get_time)(self.player) };
        let length = unsafe { (self.api.player_get_length)(self.player) };
        let position = (time >= 0).then(|| Duration::from_millis(time as u64));
        let duration = (length > 0).then(|| Duration::from_millis(length as u64));
        match unsafe { (self.api.player_get_state)(self.player) } {
            1 | 2 => self.state = PlaybackState::Loading,
            3 => {
                let (phase, deferred, state, send_pause) =
                    apply_confirmed_vlc_state(self.play_request_phase, self.deferred_pause, 3);
                self.play_request_phase = phase;
                self.deferred_pause = deferred;
                self.state = state;
                if send_pause {
                    unsafe { (self.api.player_set_pause)(self.player, 1) };
                }
            }
            4 => {
                let (phase, deferred, state, _) =
                    apply_confirmed_vlc_state(self.play_request_phase, self.deferred_pause, 4);
                self.play_request_phase = phase;
                self.deferred_pause = deferred;
                self.state = state;
            }
            5 | 6 => {
                self.deferred_pause = DeferredPauseState::None;
                self.play_request_phase = PlayRequestPhase::None;
                self.state = PlaybackState::Stopped;
            }
            7 => {
                self.deferred_pause = DeferredPauseState::None;
                self.play_request_phase = PlayRequestPhase::None;
                self.state = PlaybackState::Stopped;
                self.active_generation = 0;
                self.active_request_id = None;
                self.natural_end_pending = None;
                self.invalidate_end_callback();
                let error = self.last_error();
                update_snapshot(snapshot, |snapshot| {
                    snapshot.error = Some(error);
                    snapshot.natural_end_event = None;
                });
            }
            _ => {}
        }
        if self.state == PlaybackState::Playing && self.volume_pending {
            if let Err(error) = self.set_volume(self.volume) {
                update_snapshot(snapshot, |snapshot| snapshot.error = Some(error));
            }
        }
        let state = self.state;
        update_snapshot(snapshot, |snapshot| {
            snapshot.ready = true;
            snapshot.path = self.current_path.clone();
            snapshot.play_request_phase = self.play_request_phase;
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
    player_event_manager: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    event_attach: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        unsafe extern "C" fn(*const c_void, *mut c_void),
        *mut c_void,
    ) -> c_int,
    event_detach: unsafe extern "C" fn(
        *mut c_void,
        c_int,
        unsafe extern "C" fn(*const c_void, *mut c_void),
        *mut c_void,
    ),
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
            player_event_manager: unsafe {
                required(&library, b"libvlc_media_player_event_manager\0")?
            },
            event_attach: unsafe { required(&library, b"libvlc_event_attach\0")? },
            event_detach: unsafe { required(&library, b"libvlc_event_detach\0")? },
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

#[cfg(test)]
mod tests {
    use super::{
        DeferredPauseState, PlayRequestPhase, apply_confirmed_vlc_state, is_no_audio_output_error,
        natural_end_matches_active_request, parse_debug_loading_delay_ms, pending_play_is_ready,
        toggle_loading_pause_request,
    };
    use std::time::{Duration, Instant};

    #[test]
    fn natural_end_requires_matching_generation_and_request() {
        assert!(natural_end_matches_active_request(4, Some(17), 4, 17));
        assert!(!natural_end_matches_active_request(4, Some(17), 4, 18));
        assert!(!natural_end_matches_active_request(5, Some(17), 4, 17));
        assert!(!natural_end_matches_active_request(0, Some(17), 0, 17));
    }

    #[test]
    fn recognizes_only_the_temporary_missing_audio_output_error() {
        assert!(is_no_audio_output_error(
            "Audio output error: No active audio output"
        ));
        assert!(!is_no_audio_output_error(
            "Could not set volume: invalid value"
        ));
    }

    #[test]
    fn loading_pause_request_toggles_and_cancels() {
        assert_eq!(
            toggle_loading_pause_request(DeferredPauseState::None),
            DeferredPauseState::Requested
        );
        assert_eq!(
            toggle_loading_pause_request(DeferredPauseState::Requested),
            DeferredPauseState::None
        );
    }

    #[test]
    fn pending_pause_command_is_sent_only_once() {
        assert_eq!(
            toggle_loading_pause_request(DeferredPauseState::PauseSent),
            DeferredPauseState::ResumeSent
        );
        assert_eq!(
            toggle_loading_pause_request(DeferredPauseState::ResumeSent),
            DeferredPauseState::PauseSent
        );
    }

    #[test]
    fn play_pause_playing_paused_sequence_keeps_active_request() {
        let phase = PlayRequestPhase::Loading(17);
        let requested = toggle_loading_pause_request(DeferredPauseState::None);
        assert_eq!(requested, DeferredPauseState::Requested);

        let (playing_phase, deferred, state, send_pause) =
            apply_confirmed_vlc_state(phase, requested, 3);
        assert_eq!(playing_phase, phase);
        assert_eq!(deferred, DeferredPauseState::PauseSent);
        assert_eq!(state, super::PlaybackState::Loading);
        assert!(send_pause);

        let (paused_phase, deferred, state, send_pause) =
            apply_confirmed_vlc_state(playing_phase, deferred, 4);
        assert_eq!(paused_phase, PlayRequestPhase::Active(17));
        assert_eq!(deferred, DeferredPauseState::None);
        assert_eq!(state, super::PlaybackState::Paused);
        assert!(!send_pause);
    }

    #[test]
    fn play_pause_cancel_sequence_does_not_keep_requested_pause() {
        let first = toggle_loading_pause_request(DeferredPauseState::None);
        let canceled = toggle_loading_pause_request(first);
        assert_eq!(canceled, DeferredPauseState::None);
    }

    #[test]
    fn pending_play_replacement_and_stop_clear_old_request_pause() {
        let old = PlayRequestPhase::PendingDelay(7);
        let new = PlayRequestPhase::PendingDelay(8);
        assert_ne!(old, new);
        assert_eq!(DeferredPauseState::None, DeferredPauseState::None);
        assert_eq!(PlayRequestPhase::None, PlayRequestPhase::None);
    }

    #[test]
    fn vlc_playing_after_second_pause_resumes_normally() {
        let phase = PlayRequestPhase::Loading(23);
        let deferred = toggle_loading_pause_request(DeferredPauseState::None);
        let deferred = toggle_loading_pause_request(deferred);
        assert_eq!(deferred, DeferredPauseState::None);

        let (phase, deferred, state, send_pause) = apply_confirmed_vlc_state(phase, deferred, 3);
        assert_eq!(phase, PlayRequestPhase::Active(23));
        assert_eq!(deferred, DeferredPauseState::None);
        assert_eq!(state, super::PlaybackState::Playing);
        assert!(!send_pause);
    }

    #[test]
    fn debug_loading_delay_parser_supports_zero_and_milliseconds() {
        assert_eq!(parse_debug_loading_delay_ms("0"), 0);
        assert_eq!(parse_debug_loading_delay_ms("125"), 125);
        assert_eq!(parse_debug_loading_delay_ms("not-a-number"), 3000);
    }

    #[test]
    fn pending_play_readiness_can_be_tested_without_sleeping() {
        let now = Instant::now();
        assert!(pending_play_is_ready(now, now));
        assert!(pending_play_is_ready(now, now + Duration::from_millis(1)));
        assert!(!pending_play_is_ready(now + Duration::from_millis(1), now));
    }

    #[cfg(windows)]
    use super::load_library;

    #[cfg(windows)]
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

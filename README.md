# RetroWave Player

A compact Windows desktop media player built with Rust, eframe/egui, and a
runtime-loaded libVLC engine. The UI is native; the main player, playlist,
equalizer, and video output use separate windows.

## Features

- Add supported files with the Windows file picker or drag them onto the player
  or playlist. The playlist retains each file's full path and asynchronously
  reads title, artist, album, and duration metadata when libVLC can parse it.
- Play, pause/resume, stop, seek, previous/next, volume, mute, and playback
  speed controls use libVLC. Playback time comes from the engine rather than a
  synthetic timer.
- MP3, FLAC, WAV, MP4, MKV, and AVI files can be opened. Actual codecs depend
  on the installed VLC runtime and its plugins.
- Video is rendered in its own window, with fullscreen and Escape-to-exit.
  Audio/subtitle track selection and external SRT/ASS subtitle loading are
  exposed when the VLC runtime supports them.
- The ten-band equalizer and preamp are connected to libVLC's equalizer API.
  Equalizer controls are disabled when that API is unavailable.
- Playback history is appended to
  `%APPDATA%\RetroWave Player\history.log`.

The audio spectrum visualizer is **not implemented**: this app does not yet tap
libVLC's PCM output. The display explicitly reports that limitation rather than
showing fabricated audio activity. Duration and metadata may remain unavailable
for media the VLC demuxer cannot parse.

## Build and run

```powershell
cargo fmt --check
cargo check --offline
cargo build --offline
cargo run --release
```

The project is configured for Windows 10/11 x64. No VLC SDK import library is
needed at build time: libVLC is loaded dynamically. Install the matching x64
VLC desktop runtime before playing media. The app searches for `libvlc.dll` in
its executable directory and the standard `Program Files\VideoLAN\VLC`
locations, and expects the matching `plugins` directory beside the DLL.

For a nonstandard installation, set `LIBVLC_PATH` to `libvlc.dll` (or its
directory) and `VLC_PLUGIN_PATH` to that runtime's `plugins` directory. When
`VLC_PLUGIN_PATH` is unset, the app selects the plugins directory beside the
loaded VLC DLL and sets the environment variable before initializing libVLC. Keep
`libvlc.dll`, `libvlccore.dll`, VLC plugins, and the application on the same
architecture and compatible VLC release. A missing or incompatible runtime is
reported in the player window; audio/video playback is not emulated.

When distributing VLC with the application, review VideoLAN's applicable
libVLC, plugin, and codec licensing and redistribution terms. Codec availability
and patent obligations vary by runtime build and region.

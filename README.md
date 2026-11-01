# RetroWave Player

> **Status: Pre-release development**
>
> The first public version of RetroWave Player is expected in the near future. The release date has not been announced.

RetroWave Player is a compact desktop audio and video player inspired by the classic Winamp 2.x interface. It is built with **Rust**, **egui**, and a dynamically loaded **libVLC** media engine.

## Development status

### Implemented

- [x] Audio playback: play, pause, resume and stop.
- [x] Previous and next track controls.
- [x] Playlist with file picker and drag-and-drop support.
- [x] Playback seeking, volume and mute.
- [x] Playback speed adjustment.
- [x] Ten-band equalizer with preamp.
- [x] Separate video window and fullscreen playback.
- [x] Audio and subtitle track selection.
- [x] Custom retro-inspired interface.
- [x] Playback state handling and automatic track advancement.
- [x] Basic window lifecycle and shutdown handling.

### In progress

- [ ] Fix video viewport rendering after Windows desktop minimize/restore (`Win + D`).
- [ ] Improve resize hit zones in the video window.
- [ ] Complete manual testing of video window behavior.
- [ ] Continue reviewing playback and interface edge cases.

### Planned

- [ ] Prepare the first public release.
- [ ] Create a Windows installer.
- [ ] Continue improving stability and compatibility.
- [ ] Explore cross-platform support.

## Features

### Audio playback

- Play, pause, resume, and stop.
- Previous and next track.
- Seek through media.
- Volume control and mute.
- Playback speed adjustment from 0.5× to 2.0×.
- Playback position and duration provided by libVLC.

### Playlist

- Add multiple files using the Windows file picker.
- Drag and drop media files.
- Remove selected tracks or clear the playlist.
- Double-click a track to play it.
- Read available title, artist, album, and duration metadata.

### Equalizer

- Ten frequency bands.
- Preamp adjustment.
- Enable and disable equalization.
- Flat/reset control.
- Audio processing through libVLC's equalizer API.

### Video

- Separate video output window.
- Fullscreen playback.
- Press **Esc** to exit fullscreen.
- Audio-track selection when supported by the media.
- Subtitle-track selection and external SRT/ASS subtitle loading.

## Supported media

| Type | Formats |
|---|---|
| Audio | MP3, FLAC, WAV |
| Video | MP4, MKV, AVI |
| Subtitles | SRT, ASS |

Actual playback support depends on the installed VLC runtime, its plugins, and the codecs available in that runtime.

## Current limitations

- The audio spectrum visualizer is not implemented yet. PCM audio capture is not currently available, so the interface reports this limitation instead of displaying fabricated activity.
- Metadata and duration may be unavailable for some media files.
- Video playback, alternate audio tracks, subtitles, and less common codecs require further testing.

## Requirements

- Windows 10 or Windows 11 (64-bit).
- Rust and Cargo for building from source.
- A compatible 64-bit VLC desktop runtime for media playback.

RetroWave Player loads `libvlc.dll` dynamically. The VLC SDK import library is not required at build time.

VLC is not bundled with this repository.

## Build and run

Open PowerShell in the directory containing `Cargo.toml`:

```powershell
cargo fmt --check
cargo check --offline
cargo build --offline
cargo run --release
```

## VLC runtime configuration

The application searches for `libvlc.dll` in its executable directory and standard VideoLAN installation locations.

It automatically looks for the matching `plugins` directory beside the discovered VLC DLL when `VLC_PLUGIN_PATH` is not already configured.

For custom installations, the following environment variables are supported:

- `LIBVLC_PATH` — path to `libvlc.dll` or its containing directory.
- `VLC_PLUGIN_PATH` — path to the matching VLC plugins directory.

Make sure the application, VLC libraries, and plugins use compatible versions and the same architecture.

## Project structure

```text
src/
├── main.rs       # Application entry point and window configuration
├── app.rs        # Main UI and child windows
├── skin.rs       # Custom interface styling and controls
├── playback.rs   # libVLC integration and playback management
├── media.rs      # Media types and metadata structures
└── playlist.rs   # Playlist management
```

## License

RetroWave Player is licensed under the GNU General Public License v3.0 or later (GPL-3.0-or-later).

See the LICENSE file for the full license text.

---

*RetroWave Player — a little piece of Winamp nostalgia, rebuilt with Rust.*

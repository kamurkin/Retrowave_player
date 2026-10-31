use crate::media::{Track, format_time};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Default)]
pub struct Playlist {
    pub tracks: Vec<Track>,
    pub selected: Option<usize>,
    pub current: Option<usize>,
}

impl Playlist {
    pub fn add_paths(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> (Vec<PathBuf>, Vec<String>) {
        let mut added = Vec::new();
        let mut errors = Vec::new();
        for path in paths {
            match Track::from_path(path) {
                Ok(track) => {
                    added.push(track.path.clone());
                    self.tracks.push(track);
                }
                Err(error) => errors.push(error),
            }
        }
        (added, errors)
    }

    pub fn remove_selected(&mut self) -> Option<PathBuf> {
        let index = self.selected?;
        if index >= self.tracks.len() {
            self.selected = None;
            return None;
        }

        let removed = self.tracks.remove(index).path;
        self.selected = None;
        self.current = match self.current {
            Some(current) if current == index => None,
            Some(current) if current > index => Some(current - 1),
            other => other,
        };
        Some(removed)
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.selected = None;
        self.current = None;
    }

    pub fn total_duration(&self) -> Option<Duration> {
        self.tracks.iter().try_fold(Duration::ZERO, |total, track| {
            track.duration.map(|duration| total + duration)
        })
    }

    pub fn summary(&self) -> String {
        format!(
            "{} tracks  |  {}",
            self.tracks.len(),
            format_time(self.total_duration())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::MediaType;

    fn track(path: &str, seconds: u64) -> Track {
        Track {
            path: PathBuf::from(path),
            title: path.to_owned(),
            artist: None,
            album: None,
            duration: Some(Duration::from_secs(seconds)),
            media_type: MediaType::Audio,
        }
    }

    #[test]
    fn playlist_duration_is_unknown_until_every_track_is_parsed() {
        let mut playlist = Playlist::default();
        playlist.tracks.push(track("first.mp3", 12));
        let mut second = track("second.wav", 9);
        second.duration = None;
        playlist.tracks.push(second);
        assert_eq!(playlist.total_duration(), None);
    }

    #[test]
    fn playlist_sums_known_durations_and_keeps_paths() {
        let mut playlist = Playlist::default();
        playlist.tracks.push(track("C:\\music\\first.mp3", 12));
        playlist.tracks.push(track("D:\\mix\\second.flac", 9));

        assert_eq!(playlist.total_duration(), Some(Duration::from_secs(21)));
        assert_eq!(
            playlist.tracks[0].path,
            PathBuf::from("C:\\music\\first.mp3")
        );
    }

    #[test]
    fn removing_a_track_adjusts_the_current_index() {
        let mut playlist = Playlist::default();
        playlist.tracks.push(track("first.mp3", 1));
        playlist.tracks.push(track("second.mp3", 1));
        playlist.tracks.push(track("third.mp3", 1));
        playlist.selected = Some(0);
        playlist.current = Some(2);

        assert_eq!(playlist.remove_selected(), Some(PathBuf::from("first.mp3")));
        assert_eq!(playlist.current, Some(1));
    }
}

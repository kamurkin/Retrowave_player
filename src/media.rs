use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaType {
    Audio,
    Video,
}

impl MediaType {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "mp3" | "flac" | "wav" => Some(Self::Audio),
            "mp4" | "mkv" | "avi" => Some(Self::Video),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<Duration>,
    pub media_type: MediaType,
}

impl Track {
    pub fn from_path(path: PathBuf) -> Result<Self, String> {
        if !path.is_file() {
            return Err(format!("File does not exist: {}", path.display()));
        }
        let media_type = MediaType::from_path(&path)
            .ok_or_else(|| format!("Unsupported media format: {}", path.display()))?;
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Unknown track")
            .to_owned();

        Ok(Self {
            path,
            title,
            artist: None,
            album: None,
            duration: None,
            media_type,
        })
    }
}

pub fn format_time(time: Option<Duration>) -> String {
    match time {
        Some(time) => {
            let seconds = time.as_secs();
            format!("{:02}:{:02}", seconds / 60, seconds % 60)
        }
        None => "--:--".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_formats_case_insensitively() {
        assert_eq!(
            MediaType::from_path(Path::new("mix.FLAC")),
            Some(MediaType::Audio)
        );
        assert_eq!(
            MediaType::from_path(Path::new("movie.MKV")),
            Some(MediaType::Video)
        );
        assert_eq!(MediaType::from_path(Path::new("notes.txt")), None);
    }

    #[test]
    fn formats_missing_and_long_durations() {
        assert_eq!(format_time(None), "--:--");
        assert_eq!(format_time(Some(Duration::from_secs(3_661))), "61:01");
    }
}

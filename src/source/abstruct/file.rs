use std::{
    collections::{hash_map::Entry, HashMap},
    fmt,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, ensure};
use bytes::Bytes;
use chrono::{DateTime, Local};
use spdlog::prelude::*;
use tempfile::tempdir;
use tokio::{fs, sync::Mutex};

use crate::helper::VideoResolution;

#[derive(Clone)]
pub struct FileRef<'a> {
    pub path: Option<&'a Path>,
    pub name: String,
    pub data: Bytes,
    pub size: u64,
}

impl<'a> FileRef<'a> {
    pub async fn new(path: &'a Path) -> anyhow::Result<Self> {
        let mut ret = Self::read_to_mem(path).await?;
        ret.path = Some(path);
        Ok(ret)
    }

    pub async fn read_to_mem(path: &Path) -> anyhow::Result<Self> {
        let metadata = fs::metadata(path)
            .await
            .map_err(|err| anyhow!("failed to get file size of file '{path:?}': {err}"))?;

        let file_type = metadata.file_type();
        ensure!(
            file_type.is_file() && !file_type.is_symlink(),
            "file '{path:?}' is not a regular file"
        );

        let data = fs::read(path)
            .await
            .map_err(|err| anyhow!("failed to read file '{path:?}': {err}"))?;

        Ok(Self {
            path: None,
            name: path
                .file_name()
                .ok_or_else(|| anyhow!("failed to get file name of file '{path:?}'"))?
                .to_string_lossy()
                .into(),
            data: data.into(),
            size: metadata.len(),
        })
    }
}

impl fmt::Display for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{}' ({:?})", self.name, self.size)
    }
}

impl fmt::Debug for FileRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "'{}' ({:?}) data-len={} size={}",
            self.name,
            self.size,
            self.data.len(),
            self.size,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Playback {
    pub live_start_time: Option<DateTime<Local>>,
    pub file_path: PathBuf,
    pub format: PlaybackFormat,
}

impl fmt::Display for Playback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "playback file '{}' started at {:?}",
            self.file_path.display(),
            self.live_start_time
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".into()),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaybackFormat {
    Flv,
    Mp4,
}

impl PlaybackFormat {
    pub fn extension(&self) -> &str {
        match self {
            Self::Flv => "flv",
            Self::Mp4 => "mp4",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackLoaded<'a> {
    pub file: FileRef<'a>,
    pub resolution: VideoResolution,
}

#[derive(Debug)]
pub struct PlaybackRef<'a> {
    pub live_start_time: Option<DateTime<Local>>,
    pub(in crate::source) local_file: (&'a Path, PlaybackFormat),
    pub loaded: Mutex<HashMap<PlaybackFormat, PlaybackLoaded<'a>>>,
}

impl PlaybackRef<'_> {
    pub async fn get(&self, format: PlaybackFormat) -> anyhow::Result<PlaybackLoaded<'_>> {
        let mut loaded = self.loaded.lock().await;
        match loaded.entry(format) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),
            Entry::Vacant(entry) => {
                if self.local_file.1 == format {
                    let file = FileRef::read_to_mem(self.local_file.0).await?;
                    let resolution = crate::helper::ffprobe_resolution(self.local_file.0).await?;
                    let loaded = PlaybackLoaded { file, resolution };
                    entry.insert(loaded.clone());
                    Ok(loaded)
                } else {
                    let dir =
                        tempdir().map_err(|err| anyhow!("failed to create temp dir: {err}"))?;
                    let src = self.local_file.0;
                    let target = dir
                        .path()
                        .join(src.file_name().unwrap_or_else(|| "unknown".as_ref()))
                        .with_extension(format.extension());

                    trace!("converting playback file from '{src:?}' to '{target:?}'");
                    crate::helper::ffmpeg_copy(src, &target).await?;
                    trace!("converting done.");

                    let mut converted = FileRef::read_to_mem(&target).await?;
                    converted.path = None; // The temp file will be deleted when dropped
                    let resolution = crate::helper::ffprobe_resolution(&target).await?;
                    let loaded = PlaybackLoaded {
                        file: converted,
                        resolution,
                    };
                    entry.insert(loaded.clone());
                    Ok(loaded)
                }
            }
        }
    }
}

impl fmt::Display for PlaybackRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "playback file {:?} started at {:?}",
            self.local_file.0,
            self.live_start_time
                .map(|t| t.to_string())
                .unwrap_or_else(|| "unknown".into()),
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub file_path: PathBuf,
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "document file '{}' started", self.file_path.display())
    }
}

#[derive(Clone, Debug)]
pub struct DocumentRef<'a> {
    pub file: FileRef<'a>,
}

impl fmt::Display for DocumentRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "document file {}", self.file)
    }
}

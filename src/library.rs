use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts",
];

#[derive(Debug, Clone, Serialize)]
pub struct MediaFile {
    pub filename: String,
    pub path: String,
    /// Which base dir this file is in ("media" or "downloads")
    pub source: String,
    pub size_bytes: u64,
    pub size_display: String,
    pub has_subs: bool,
    pub srt_path: Option<String>,
    /// Video codec (e.g. "h264", "hevc", "vp9")
    pub video_codec: String,
    /// Audio codec (e.g. "aac", "ac3", "opus")
    pub audio_codec: String,
    /// Resolution (e.g. "1920x1080")
    pub resolution: String,
    /// Container format (e.g. "mkv", "mp4")
    pub container: String,
    /// Whether this file can be played natively in a browser
    pub browser_playable: bool,
    /// HLS playlist path if a transcoded version exists
    pub hls_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StorageInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub free_bytes: u64,
    pub total_display: String,
    pub used_display: String,
    pub free_display: String,
    pub used_pct: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    pub files: Vec<MediaFile>,
    pub storage: StorageInfo,
}

impl Library {
    pub async fn scan(media_dir: &Path, download_dir: &Path) -> Self {
        let mut files = Vec::new();

        // Scan both directories
        Self::scan_dir(media_dir, "media", media_dir, &mut files).await;
        if download_dir != media_dir {
            Self::scan_dir(download_dir, "downloads", media_dir, &mut files).await;
        }

        files.sort_by(|a, b| a.filename.to_lowercase().cmp(&b.filename.to_lowercase()));

        let storage = get_storage_info(media_dir);

        Library { files, storage }
    }

    async fn scan_dir(dir: &Path, source: &str, media_dir: &Path, files: &mut Vec<MediaFile>) {
        Self::scan_recursive(dir, dir, source, media_dir, files).await;
    }

    async fn scan_recursive(base: &Path, dir: &Path, source: &str, media_dir: &Path, files: &mut Vec<MediaFile>) {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => return,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                Box::pin(Self::scan_recursive(base, &path, source, media_dir, files)).await;
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
                    let size = tokio::fs::metadata(&path)
                        .await
                        .map(|m| m.len())
                        .unwrap_or(0);
                    let rel_path = path
                        .strip_prefix(base)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .to_string();
                    let srt = path.with_extension("srt");
                    let has_subs = srt.exists();
                    let srt_rel = if has_subs {
                        Some(
                            srt.strip_prefix(base)
                                .unwrap_or(&srt)
                                .to_string_lossy()
                                .to_string(),
                        )
                    } else {
                        None
                    };
                    // For serving, prefix with source dir so the /media route can find it
                    let serve_path = if source == "downloads" {
                        // We'll add a /downloads nest route
                        format!("dl/{}", rel_path)
                    } else {
                        rel_path
                    };
                    let ext_lower = ext.to_lowercase();
                    let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();

                    // Probe codec info
                    let probe = probe_video(&path).await;
                    let video_codec = probe.0;
                    let audio_codec = probe.1;
                    let resolution = probe.2;
                    let container = ext_lower.clone();

                    // Browser can play H.264/VP8/VP9 in MP4/WebM containers
                    let browser_playable = matches!(container.as_str(), "mp4" | "webm" | "m4v")
                        && matches!(video_codec.as_str(), "h264" | "vp8" | "vp9" | "");

                    let hls_path = crate::transcode::hls_playlist_path(media_dir, &fname)
                        .map(|p| {
                            p.strip_prefix(media_dir)
                                .unwrap_or(&p)
                                .to_string_lossy()
                                .to_string()
                        });

                    files.push(MediaFile {
                        filename: fname,
                        path: serve_path,
                        source: source.to_string(),
                        size_bytes: size,
                        size_display: format_size(size),
                        has_subs,
                        srt_path: srt_rel.map(|p| {
                            if source == "downloads" {
                                format!("dl/{}", p)
                            } else {
                                p
                            }
                        }),
                        video_codec,
                        audio_codec,
                        resolution,
                        container,
                        browser_playable,
                        hls_path,
                    });
                }
            }
        }
    }
}

fn get_storage_info(path: &Path) -> StorageInfo {
    // Use statvfs to get filesystem info
    use std::ffi::CString;
    let c_path = CString::new(path.to_string_lossy().as_bytes()).unwrap_or_default();
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };

    if ret == 0 {
        let total = stat.f_blocks as u64 * stat.f_frsize as u64;
        let free = stat.f_bavail as u64 * stat.f_frsize as u64;
        let used = total - free;
        let used_pct = if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        StorageInfo {
            total_bytes: total,
            used_bytes: used,
            free_bytes: free,
            total_display: format_size(total),
            used_display: format_size(used),
            free_display: format_size(free),
            used_pct,
        }
    } else {
        StorageInfo {
            total_bytes: 0,
            used_bytes: 0,
            free_bytes: 0,
            total_display: "?".into(),
            used_display: "?".into(),
            free_display: "?".into(),
            used_pct: 0.0,
        }
    }
}

/// Probe video file for codec and resolution info via ffprobe
async fn probe_video(path: &Path) -> (String, String, String) {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-show_entries", "stream=codec_name,codec_type,width,height",
            "-of", "csv=p=0",
            &path.to_string_lossy(),
        ])
        .output()
        .await;

    let output = match output {
        Ok(o) => o,
        Err(_) => return ("unknown".into(), "unknown".into(), "".into()),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut video_codec = String::new();
    let mut audio_codec = String::new();
    let mut resolution = String::new();

    for line in text.lines() {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() >= 2 {
            let codec = parts[0].trim();
            let ctype = parts[1].trim();
            if ctype == "video" && video_codec.is_empty() {
                video_codec = codec.to_string();
                if parts.len() >= 4 {
                    let w = parts[2].trim();
                    let h = parts[3].trim();
                    if !w.is_empty() && !h.is_empty() {
                        resolution = format!("{}x{}", w, h);
                    }
                }
            } else if ctype == "audio" && audio_codec.is_empty() {
                audio_codec = codec.to_string();
            }
        }
    }

    if video_codec.is_empty() { video_codec = "unknown".into(); }
    if audio_codec.is_empty() { audio_codec = "unknown".into(); }

    (video_codec, audio_codec, resolution)
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// Re-scan the library every 30 seconds
pub async fn watch_loop(state: Arc<crate::AppState>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let new_lib = Library::scan(&state.media_dir, &state.download_dir).await;
        let mut lib = state.library.write().await;
        *lib = new_lib;
    }
}

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts",
];

const SUB_EXTENSIONS: &[&str] = &["srt", "vtt", "ass", "ssa", "sub", "idx"];

#[derive(Debug, Clone, Serialize)]
pub struct SubTrack {
    pub path: String,
    pub label: String,
    pub lang: String,
    pub format: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MediaFile {
    pub filename: String,
    pub path: String,
    pub source: String,
    pub size_bytes: u64,
    pub size_display: String,
    pub has_subs: bool,
    pub subtitle_tracks: Vec<SubTrack>,
    pub video_codec: String,
    pub audio_codec: String,
    pub resolution: String,
    pub container: String,
    pub browser_playable: bool,
    pub hls_path: Option<String>,
    /// Embedded subtitle tracks detected by ffprobe
    pub embedded_subs: Vec<String>,
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
                    let serve_path = if source == "downloads" {
                        format!("dl/{}", rel_path)
                    } else {
                        rel_path
                    };
                    let ext_lower = ext.to_lowercase();
                    let fname = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();

                    // Find external subtitle files
                    let subtitle_tracks = find_subtitle_tracks(
                        &path, base, source, &stem,
                    ).await;
                    let has_subs = !subtitle_tracks.is_empty();

                    // Probe codec info + embedded subs
                    let (video_codec, audio_codec, resolution, embedded_subs) =
                        probe_video_full(&path).await;
                    let container = ext_lower.clone();

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
                        has_subs: has_subs || !embedded_subs.is_empty(),
                        subtitle_tracks,
                        video_codec,
                        audio_codec,
                        resolution,
                        container,
                        browser_playable,
                        hls_path,
                        embedded_subs,
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

/// Find all external subtitle files matching a video's stem
async fn find_subtitle_tracks(
    video_path: &Path,
    base: &Path,
    source: &str,
    stem: &str,
) -> Vec<SubTrack> {
    let mut tracks = Vec::new();
    let parent = match video_path.parent() {
        Some(p) => p,
        None => return tracks,
    };

    // Check files in same directory and "Subs"/"Subtitles" subdirectories
    let dirs_to_check: Vec<PathBuf> = vec![
        parent.to_path_buf(),
        parent.join("Subs"),
        parent.join("subs"),
        parent.join("Subtitles"),
        parent.join("subtitles"),
    ];

    for dir in dirs_to_check {
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let p = entry.path();
            if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                let ext_lower = ext.to_lowercase();
                if !SUB_EXTENSIONS.contains(&ext_lower.as_str()) {
                    continue;
                }
                let fname = p.file_name().unwrap_or_default().to_string_lossy().to_string();

                // Match if the subtitle filename starts with the video stem
                // or if it's in a Subs directory (include all)
                let in_subs_dir = dir != parent.to_path_buf();
                if !fname.to_lowercase().starts_with(&stem.to_lowercase()) && !in_subs_dir {
                    continue;
                }

                let lang = guess_language(&fname, stem);
                let label = if lang == "und" {
                    fname.clone()
                } else {
                    format!("{} ({})", lang_name(&lang), ext_lower)
                };

                let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().to_string();
                let serve = if source == "downloads" {
                    format!("dl/{}", rel)
                } else {
                    rel
                };

                tracks.push(SubTrack {
                    path: serve,
                    label,
                    lang: lang.clone(),
                    format: ext_lower,
                });
            }
        }
    }

    // Sort: English first, then alphabetical
    tracks.sort_by(|a, b| {
        let a_en = a.lang == "en" || a.lang == "eng";
        let b_en = b.lang == "en" || b.lang == "eng";
        b_en.cmp(&a_en).then(a.label.cmp(&b.label))
    });

    tracks
}

/// Guess subtitle language from filename patterns
fn guess_language(filename: &str, video_stem: &str) -> String {
    let lower = filename.to_lowercase();
    let suffix = lower
        .strip_prefix(&video_stem.to_lowercase())
        .unwrap_or(&lower)
        .trim_start_matches('.');

    // Common patterns: movie.en.srt, movie.english.srt, English.srt
    let lang_part = suffix.split('.').next().unwrap_or("");

    match lang_part {
        "en" | "eng" | "english" => "en".into(),
        "es" | "spa" | "spanish" => "es".into(),
        "fr" | "fre" | "french" => "fr".into(),
        "de" | "ger" | "german" => "de".into(),
        "it" | "ita" | "italian" => "it".into(),
        "pt" | "por" | "portuguese" => "pt".into(),
        "nl" | "dut" | "dutch" => "nl".into(),
        "ru" | "rus" | "russian" => "ru".into(),
        "ja" | "jpn" | "japanese" => "ja".into(),
        "ko" | "kor" | "korean" => "ko".into(),
        "zh" | "chi" | "chinese" => "zh".into(),
        "ar" | "ara" | "arabic" => "ar".into(),
        "hi" | "hin" | "hindi" => "hi".into(),
        "sv" | "swe" | "swedish" => "sv".into(),
        "no" | "nor" | "norwegian" => "no".into(),
        "da" | "dan" | "danish" => "da".into(),
        "fi" | "fin" | "finnish" => "fi".into(),
        "pl" | "pol" | "polish" => "pl".into(),
        "tr" | "tur" | "turkish" => "tr".into(),
        "sdh" => "en".into(), // SDH is usually English
        s if s.len() == 2 || s.len() == 3 => s.to_string(),
        _ => {
            // Check if the whole filename (for Subs/ directories) is a language
            let name_no_ext = filename.rsplit('.').skip(1).collect::<Vec<_>>().join(".");
            let name_lower = name_no_ext.to_lowercase();
            if name_lower.contains("english") { return "en".into(); }
            if name_lower.contains("spanish") { return "es".into(); }
            if name_lower.contains("french") { return "fr".into(); }
            "und".into()
        }
    }
}

fn lang_name(code: &str) -> &str {
    match code {
        "en" | "eng" => "English",
        "es" | "spa" => "Spanish",
        "fr" | "fre" => "French",
        "de" | "ger" => "German",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "nl" | "dut" => "Dutch",
        "ru" | "rus" => "Russian",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "zh" | "chi" => "Chinese",
        "ar" | "ara" => "Arabic",
        "hi" | "hin" => "Hindi",
        "sv" | "swe" => "Swedish",
        "no" | "nor" => "Norwegian",
        "da" | "dan" => "Danish",
        "fi" | "fin" => "Finnish",
        "pl" | "pol" => "Polish",
        "tr" | "tur" => "Turkish",
        _ => code,
    }
}

/// Probe video file for codec, resolution, and embedded subtitle tracks
async fn probe_video_full(path: &Path) -> (String, String, String, Vec<String>) {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-show_entries", "stream=codec_name,codec_type,width,height:stream_tags=language,title",
            "-of", "csv=p=0",
            &path.to_string_lossy(),
        ])
        .output()
        .await;

    let output = match output {
        Ok(o) => o,
        Err(_) => return ("unknown".into(), "unknown".into(), "".into(), vec![]),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut video_codec = String::new();
    let mut audio_codec = String::new();
    let mut resolution = String::new();
    let mut embedded_subs = Vec::new();

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
                    if !w.is_empty() && !h.is_empty() && w != "N/A" {
                        resolution = format!("{}x{}", w, h);
                    }
                }
            } else if ctype == "audio" && audio_codec.is_empty() {
                audio_codec = codec.to_string();
            } else if ctype == "subtitle" {
                // Try to get language tag
                let lang = parts.get(4).unwrap_or(&"und").trim();
                let title = parts.get(5).unwrap_or(&"").trim();
                let label = if !title.is_empty() {
                    format!("{} ({})", title, codec)
                } else if lang != "und" && !lang.is_empty() {
                    format!("{} ({})", lang_name(lang), codec)
                } else {
                    format!("Track {} ({})", embedded_subs.len() + 1, codec)
                };
                embedded_subs.push(label);
            }
        }
    }

    if video_codec.is_empty() { video_codec = "unknown".into(); }
    if audio_codec.is_empty() { audio_codec = "unknown".into(); }

    (video_codec, audio_codec, resolution, embedded_subs)
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

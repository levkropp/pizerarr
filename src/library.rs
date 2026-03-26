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
        Self::scan_dir(media_dir, "media", &mut files).await;
        if download_dir != media_dir {
            Self::scan_dir(download_dir, "downloads", &mut files).await;
        }

        files.sort_by(|a, b| a.filename.to_lowercase().cmp(&b.filename.to_lowercase()));

        let storage = get_storage_info(media_dir);

        Library { files, storage }
    }

    async fn scan_dir(dir: &Path, source: &str, files: &mut Vec<MediaFile>) {
        Self::scan_recursive(dir, dir, source, files).await;
    }

    async fn scan_recursive(base: &Path, dir: &Path, source: &str, files: &mut Vec<MediaFile>) {
        let mut entries = match tokio::fs::read_dir(dir).await {
            Ok(e) => e,
            Err(_) => return,
        };

        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.is_dir() {
                Box::pin(Self::scan_recursive(base, &path, source, files)).await;
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
                    files.push(MediaFile {
                        filename: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string(),
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

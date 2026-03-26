use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
pub struct TranscodeJob {
    pub source: String,
    pub hls_dir: String,
    pub playlist: String,
    pub status: TranscodeStatus,
    pub progress_pct: f64,
    pub log: Vec<String>,
    #[serde(skip)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum TranscodeStatus {
    Queued,
    Running,
    Done,
    Failed(String),
}

pub type TranscodeMap = Arc<RwLock<HashMap<String, TranscodeJob>>>;

pub fn new_map() -> TranscodeMap {
    Arc::new(RwLock::new(HashMap::new()))
}

fn hls_dir_for(media_dir: &Path, filename: &str) -> PathBuf {
    let stem = Path::new(filename)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let safe: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    media_dir.join("hls").join(safe)
}

pub fn hls_playlist_path(media_dir: &Path, filename: &str) -> Option<PathBuf> {
    let dir = hls_dir_for(media_dir, filename);
    let playlist = dir.join("index.m3u8");
    if playlist.exists() {
        Some(playlist)
    } else {
        None
    }
}

pub async fn queue_transcode(
    map: &TranscodeMap,
    media_dir: &Path,
    source_path: &Path,
    filename: &str,
) {
    let hls_dir = hls_dir_for(media_dir, filename);
    let playlist_rel = format!(
        "hls/{}/index.m3u8",
        hls_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );

    {
        let jobs = map.read().await;
        if let Some(job) = jobs.get(filename) {
            if job.status == TranscodeStatus::Done || job.status == TranscodeStatus::Running {
                return;
            }
        }
    }

    if hls_dir.join("index.m3u8").exists() {
        let mut jobs = map.write().await;
        jobs.insert(
            filename.to_string(),
            TranscodeJob {
                source: source_path.to_string_lossy().to_string(),
                hls_dir: hls_dir.to_string_lossy().to_string(),
                playlist: playlist_rel,
                status: TranscodeStatus::Done,
                progress_pct: 100.0,
                log: vec!["Already converted.".into()],
                pid: None,
            },
        );
        return;
    }

    let job = TranscodeJob {
        source: source_path.to_string_lossy().to_string(),
        hls_dir: hls_dir.to_string_lossy().to_string(),
        playlist: playlist_rel,
        status: TranscodeStatus::Queued,
        progress_pct: 0.0,
        log: vec!["Queued for conversion...".into()],
        pid: None,
    };

    map.write().await.insert(filename.to_string(), job);

    let map = map.clone();
    let filename = filename.to_string();
    let source = source_path.to_owned();

    tokio::spawn(async move {
        run_transcode(&map, &source, &hls_dir, &filename).await;
    });
}

async fn run_transcode(map: &TranscodeMap, source: &Path, hls_dir: &Path, filename: &str) {
    if let Err(e) = tokio::fs::create_dir_all(hls_dir).await {
        set_failed(map, filename, &format!("mkdir failed: {}", e)).await;
        return;
    }

    set_status(map, filename, TranscodeStatus::Running).await;
    append_log(map, filename, "Starting ffmpeg...").await;

    let duration = get_duration(source).await.unwrap_or(0.0);
    append_log(
        map,
        filename,
        &format!("Source duration: {:.0}s ({:.1} min)", duration, duration / 60.0),
    )
    .await;

    let playlist = hls_dir.join("index.m3u8");
    let progress_file = hls_dir.join("progress.log");

    let mut child = match tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &source.to_string_lossy(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-crf",
            "23",
            "-maxrate",
            "4M",
            "-bufsize",
            "8M",
            "-vf",
            "scale='min(1920,iw)':'min(1080,ih)':force_original_aspect_ratio=decrease",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-ac",
            "2",
            "-f",
            "hls",
            "-hls_time",
            "10",
            "-hls_list_size",
            "0",
            "-hls_segment_filename",
            &hls_dir.join("seg%04d.ts").to_string_lossy(),
            "-progress",
            &progress_file.to_string_lossy(),
            "-y",
            &playlist.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            set_failed(map, filename, &format!("Failed to start ffmpeg: {}", e)).await;
            return;
        }
    };

    // Store PID for cancellation
    if let Some(pid) = child.id() {
        if let Some(job) = map.write().await.get_mut(filename) {
            job.pid = Some(pid);
        }
    }

    // Read stderr for log output
    if let Some(stderr) = child.stderr.take() {
        let map_clone = map.clone();
        let fname = filename.to_string();
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_string();
                if !trimmed.is_empty() {
                    append_log(&map_clone, &fname, &trimmed).await;
                }
            }
        });
    }

    // Poll the progress file for time updates
    let map_clone = map.clone();
    let fname = filename.to_string();
    let pfile = progress_file.clone();
    let poll_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            if let Ok(text) = tokio::fs::read_to_string(&pfile).await {
                // Find last out_time_us line
                let mut last_us: f64 = 0.0;
                let mut speed = String::new();
                for line in text.lines() {
                    if let Some(val) = line.strip_prefix("out_time_us=") {
                        if let Ok(us) = val.trim().parse::<f64>() {
                            last_us = us;
                        }
                    }
                    if let Some(val) = line.strip_prefix("speed=") {
                        speed = val.trim().to_string();
                    }
                }
                if last_us > 0.0 && duration > 0.0 {
                    let secs = last_us / 1_000_000.0;
                    let pct = (secs / duration * 100.0).min(99.9);
                    if let Some(job) = map_clone.write().await.get_mut(&fname) {
                        job.progress_pct = pct;
                    }
                    let msg = format!(
                        "Progress: {:.1}% ({:.0}s / {:.0}s) speed={}",
                        pct, secs, duration, speed
                    );
                    append_log(&map_clone, &fname, &msg).await;
                }
            }
        }
    });

    let status = child.wait().await;
    poll_handle.abort();

    match status {
        Ok(s) if s.success() => {
            if let Some(job) = map.write().await.get_mut(filename) {
                job.status = TranscodeStatus::Done;
                job.progress_pct = 100.0;
                job.log.push("Conversion complete!".into());
            }
            tracing::info!("transcode complete: {}", filename);
        }
        Ok(s) => {
            set_failed(map, filename, &format!("ffmpeg exited with: {}", s)).await;
        }
        Err(e) => {
            set_failed(map, filename, &format!("ffmpeg error: {}", e)).await;
        }
    }

    // Clean up progress file
    let _ = tokio::fs::remove_file(&progress_file).await;
}

async fn set_status(map: &TranscodeMap, filename: &str, status: TranscodeStatus) {
    if let Some(job) = map.write().await.get_mut(filename) {
        job.status = status;
    }
}

async fn set_failed(map: &TranscodeMap, filename: &str, msg: &str) {
    if let Some(job) = map.write().await.get_mut(filename) {
        job.status = TranscodeStatus::Failed(msg.to_string());
        job.log.push(format!("ERROR: {}", msg));
    }
}

async fn append_log(map: &TranscodeMap, filename: &str, line: &str) {
    if let Some(job) = map.write().await.get_mut(filename) {
        // Keep last 100 lines
        if job.log.len() > 100 {
            job.log.drain(..job.log.len() - 80);
        }
        job.log.push(line.to_string());
    }
}

async fn get_duration(path: &Path) -> Option<f64> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            &path.to_string_lossy(),
        ])
        .output()
        .await
        .ok()?;
    let s = String::from_utf8_lossy(&output.stdout);
    s.trim().parse().ok()
}

pub async fn cancel(map: &TranscodeMap, media_dir: &Path, filename: &str) {
    let (pid, hls_dir) = {
        let mut jobs = map.write().await;
        if let Some(job) = jobs.get_mut(filename) {
            let pid = job.pid.take();
            let dir = job.hls_dir.clone();
            job.status = TranscodeStatus::Failed("Cancelled by user".into());
            job.log.push("Cancelled.".into());
            (pid, dir)
        } else {
            return;
        }
    };

    // Kill ffmpeg process
    if let Some(pid) = pid {
        let _ = tokio::process::Command::new("kill")
            .arg(pid.to_string())
            .status()
            .await;
    }

    // Clean up partial HLS files
    let _ = tokio::fs::remove_dir_all(&hls_dir).await;

    // Remove from job map so it can be retried
    map.write().await.remove(filename);
}

pub async fn auto_transcode_loop(state: Arc<crate::AppState>) {
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    loop {
        let lib = state.library.read().await;
        for file in &lib.files {
            let source_path = if file.source == "downloads" {
                state
                    .download_dir
                    .join(file.path.strip_prefix("dl/").unwrap_or(&file.path))
            } else {
                state.media_dir.join(&file.path)
            };
            if hls_playlist_path(&state.media_dir, &file.filename).is_none() {
                queue_transcode(
                    &state.transcode_jobs,
                    &state.media_dir,
                    &source_path,
                    &file.filename,
                )
                .await;
            }
        }
        drop(lib);
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

use crate::search;
use crate::subs;
use crate::tmdb;
use crate::torrent;
use crate::transcode;
use crate::AppState;

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Json;
use axum::Router;
use rust_embed::Embed;
use serde::Deserialize;
use std::sync::Arc;
use tower_http::services::ServeDir;

#[derive(Embed)]
#[folder = "static/"]
struct StaticAssets;

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/api/search/torrents", get(api_search_torrents))
        .route("/api/search/meta", get(api_search_meta))
        .route("/api/search/subs", get(api_search_subs))
        .route("/api/trending", get(api_trending))
        .route("/api/torrents", get(api_list_torrents))
        .route("/api/torrents", post(api_add_torrent))
        .route("/api/torrents/{id}", delete(api_delete_torrent))
        .route("/api/magnet/1337x", get(api_fetch_1337x_magnet))
        .route("/api/library", get(api_library))
        .route("/api/subs/download", post(api_download_sub))
        .route("/api/transcode", post(api_start_transcode))
        .route("/api/transcode/status", get(api_transcode_status))
        .route("/api/transcode/cancel", post(api_cancel_transcode))
        .route("/api/subs/vtt", get(api_srt_to_vtt))
        .route("/static/{*path}", get(serve_static))
        .nest_service("/media/dl", ServeDir::new(state.download_dir.clone()))
        .nest_service("/media", ServeDir::new(state.media_dir.clone()))
        .with_state(state)
}

async fn index_page() -> impl IntoResponse {
    match StaticAssets::get("index.html") {
        Some(content) => Html(
            std::str::from_utf8(content.data.as_ref())
                .unwrap_or("")
                .to_string(),
        )
        .into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn serve_static(Path(path): Path<String>) -> impl IntoResponse {
    match StaticAssets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                content.data.to_vec(),
            )
                .into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn api_search_torrents(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Json<Vec<search::SearchResult>> {
    let client = reqwest::Client::new();
    let results = search::search_all(&client, &params.q).await;
    Json(results)
}

async fn api_search_meta(
    Query(params): Query<SearchQuery>,
) -> Json<Vec<tmdb::MetadataItem>> {
    let client = reqwest::Client::new();
    let results = tmdb::search_multi(&client, &params.q)
        .await
        .unwrap_or_default();
    Json(results)
}

#[derive(Deserialize)]
struct SubsQuery {
    q: String,
    lang: Option<String>,
}

async fn api_search_subs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SubsQuery>,
) -> Response {
    let lang = params.lang.as_deref().unwrap_or("eng");
    match subs::search(&state.http_client, &state.subs_cache, &params.q, lang).await {
        Ok(results) => Json(results).into_response(),
        Err(e) => {
            tracing::error!("subtitle search error: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
struct SaveSubBody {
    /// OpenSubtitles download URL (server fetches + decompresses)
    download_url: String,
    /// Path relative to media dir where the video lives
    video_path: String,
}

async fn api_download_sub(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveSubBody>,
) -> Response {
    let video = if let Some(rel) = body.video_path.strip_prefix("dl/") {
        state.download_dir.join(rel)
    } else {
        state.media_dir.join(&body.video_path)
    };
    let srt_path = video.with_extension("srt");

    match subs::download_to(&state.http_client, &body.download_url, &srt_path).await {
        Ok(()) => {}
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    Json(serde_json::json!({
        "status": "ok",
        "path": srt_path.to_string_lossy()
    }))
    .into_response()
}

async fn api_trending(State(state): State<Arc<AppState>>) -> Json<Vec<tmdb::MetadataItem>> {
    let client = reqwest::Client::new();
    let results = tmdb::get_trending(&client, &state.download_dir).await;
    Json(results)
}

async fn api_list_torrents(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<torrent::TorrentStatus>> {
    Json(torrent::list_torrents(&state.torrent_session, &state.torrent_meta).await)
}

#[derive(Deserialize)]
struct AddTorrentBody {
    magnet: String,
    title: Option<String>,
    poster_url: Option<String>,
}

async fn api_add_torrent(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddTorrentBody>,
) -> Response {
    let meta = torrent::TorrentMeta {
        title: body.title.unwrap_or_default(),
        poster_url: body.poster_url,
    };
    match torrent::add_magnet(&state.torrent_session, &body.magnet, meta, &state.torrent_meta).await
    {
        Ok(id) => Json(serde_json::json!({ "id": id, "status": "added" })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_delete_torrent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<usize>,
) -> Response {
    match torrent::delete_torrent(&state.torrent_session, id, &state.torrent_meta).await {
        Ok(()) => Json(serde_json::json!({ "status": "deleted" })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct MagnetQuery {
    url: String,
}

async fn api_fetch_1337x_magnet(Query(params): Query<MagnetQuery>) -> Response {
    let client = reqwest::Client::new();
    match search::fetch_1337x_magnet(&client, &params.url).await {
        Ok(magnet) => Json(serde_json::json!({ "magnet": magnet })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn api_library(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let lib = state.library.read().await;
    Json(lib.clone())
}

// --- SRT to VTT conversion ---

#[derive(Deserialize)]
struct VttQuery {
    path: String,
}

async fn api_srt_to_vtt(
    State(state): State<Arc<AppState>>,
    Query(params): Query<VttQuery>,
) -> Response {
    let file_path = if let Some(rel) = params.path.strip_prefix("dl/") {
        state.download_dir.join(rel)
    } else {
        state.media_dir.join(&params.path)
    };

    let content = match tokio::fs::read_to_string(&file_path).await {
        Ok(c) => c,
        Err(_) => {
            return (StatusCode::NOT_FOUND, "subtitle file not found").into_response();
        }
    };

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let vtt = match ext.as_str() {
        "vtt" => content,
        "srt" => srt_to_vtt(&content),
        "ass" | "ssa" => ass_to_vtt(&content),
        _ => srt_to_vtt(&content),
    };

    (
        [(header::CONTENT_TYPE, "text/vtt; charset=utf-8")],
        vtt,
    )
        .into_response()
}

fn srt_to_vtt(srt: &str) -> String {
    let mut vtt = String::from("WEBVTT\n\n");
    for line in srt.lines() {
        // SRT uses comma for milliseconds, VTT uses dot
        if line.contains(" --> ") {
            vtt.push_str(&line.replace(',', "."));
        } else {
            vtt.push_str(line);
        }
        vtt.push('\n');
    }
    vtt
}

fn ass_to_vtt(ass: &str) -> String {
    let mut vtt = String::from("WEBVTT\n\n");
    let mut count = 1;
    for line in ass.lines() {
        if !line.starts_with("Dialogue:") {
            continue;
        }
        // Format: Dialogue: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text
        let parts: Vec<&str> = line.splitn(10, ',').collect();
        if parts.len() < 10 {
            continue;
        }
        let start = ass_time_to_vtt(parts[1].trim());
        let end = ass_time_to_vtt(parts[2].trim());
        let text = parts[9]
            .replace("\\N", "\n")
            .replace("\\n", "\n")
            .replace("{\\an8}", "")
            .replace("{\\pos(", "");
        // Strip other ASS override tags
        let text = strip_ass_tags(&text);
        if text.trim().is_empty() {
            continue;
        }
        vtt.push_str(&format!("{}\n{} --> {}\n{}\n\n", count, start, end, text));
        count += 1;
    }
    vtt
}

fn ass_time_to_vtt(t: &str) -> String {
    // ASS: H:MM:SS.CC -> VTT: HH:MM:SS.mmm
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() == 3 {
        let h = parts[0];
        let m = parts[1];
        let sec_parts: Vec<&str> = parts[2].split('.').collect();
        let s = sec_parts.first().unwrap_or(&"00");
        let cs = sec_parts.get(1).unwrap_or(&"00");
        let ms: u32 = cs.parse::<u32>().unwrap_or(0) * 10;
        format!("{:0>2}:{:0>2}:{:0>2}.{:03}", h, m, s, ms)
    } else {
        t.to_string()
    }
}

fn strip_ass_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '{' {
            in_tag = true;
        } else if c == '}' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out
}

// --- Transcode ---

#[derive(Deserialize)]
struct TranscodeBody {
    filename: String,
    path: String,
    source: String,
}

async fn api_start_transcode(
    State(state): State<Arc<AppState>>,
    Json(body): Json<TranscodeBody>,
) -> Response {
    let source_path = if body.source == "downloads" {
        state.download_dir.join(
            body.path.strip_prefix("dl/").unwrap_or(&body.path),
        )
    } else {
        state.media_dir.join(&body.path)
    };

    transcode::queue_transcode(
        &state.transcode_jobs,
        &state.media_dir,
        &source_path,
        &body.filename,
    )
    .await;

    Json(serde_json::json!({ "status": "queued" })).into_response()
}

#[derive(Deserialize)]
struct TranscodeStatusQuery {
    filename: String,
}

async fn api_transcode_status(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TranscodeStatusQuery>,
) -> Response {
    let jobs = state.transcode_jobs.read().await;
    if let Some(job) = jobs.get(&params.filename) {
        Json(job.clone()).into_response()
    } else {
        // Check if HLS already exists
        if transcode::hls_playlist_path(&state.media_dir, &params.filename).is_some() {
            Json(serde_json::json!({
                "status": "Done",
                "progress_pct": 100.0,
            }))
            .into_response()
        } else {
            Json(serde_json::json!({ "status": "none" })).into_response()
        }
    }
}

#[derive(Deserialize)]
struct CancelBody {
    filename: String,
}

async fn api_cancel_transcode(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CancelBody>,
) -> Response {
    transcode::cancel(&state.transcode_jobs, &state.media_dir, &body.filename).await;
    Json(serde_json::json!({ "status": "cancelled" })).into_response()
}

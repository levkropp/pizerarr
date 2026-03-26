use crate::search;
use crate::subs;
use crate::tmdb;
use crate::torrent;
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
    /// Raw .srt content (browser fetches + decompresses from OpenSubtitles)
    srt_content: String,
    /// Path relative to media dir where the video lives
    video_path: String,
}

async fn api_download_sub(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SaveSubBody>,
) -> Response {
    // dl/ prefix means the file is in download_dir
    let video = if let Some(rel) = body.video_path.strip_prefix("dl/") {
        state.download_dir.join(rel)
    } else {
        state.media_dir.join(&body.video_path)
    };
    let srt_path = video.with_extension("srt");

    match tokio::fs::write(&srt_path, &body.srt_content).await {
        Ok(()) => Json(serde_json::json!({
            "status": "ok",
            "path": srt_path.to_string_lossy()
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
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

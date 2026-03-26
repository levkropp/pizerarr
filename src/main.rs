mod library;
mod routes;
mod search;
mod subs;
mod tmdb;
mod torrent;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct AppState {
    pub media_dir: PathBuf,
    pub download_dir: PathBuf,
    pub torrent_session: Arc<librqbit::Session>,
    pub library: RwLock<library::Library>,
    pub torrent_meta: torrent::MetaMap,
    pub http_client: reqwest::Client,
    pub subs_cache: subs::SubsCache,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let media_dir = PathBuf::from(
        std::env::var("PIZERARR_MEDIA_DIR").unwrap_or_else(|_| "./media".to_string()),
    );
    let download_dir = PathBuf::from(
        std::env::var("PIZERARR_DOWNLOAD_DIR").unwrap_or_else(|_| "./downloads".to_string()),
    );
    let port: u16 = std::env::var("PIZERARR_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    tokio::fs::create_dir_all(&media_dir).await?;
    tokio::fs::create_dir_all(&download_dir).await?;

    let session = librqbit::Session::new(download_dir.clone()).await?;
    let library = library::Library::scan(&media_dir, &download_dir).await;
    let http_client = reqwest::Client::new();
    let subs_cache = subs::new_cache();

    let state = Arc::new(AppState {
        media_dir,
        download_dir,
        torrent_session: session,
        library: RwLock::new(library),
        torrent_meta: torrent::new_meta_map(),
        http_client,
        subs_cache,
    });

    // Library watcher
    let watch_state = state.clone();
    tokio::spawn(async move {
        library::watch_loop(watch_state).await;
    });

    let app = routes::build_router(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("pizerarr listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

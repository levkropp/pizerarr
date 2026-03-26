mod library;
mod routes;
mod search;
mod subs;
mod tmdb;
mod torrent;
mod transcode;

use axum::response::Redirect;
use axum::routing::get;
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
    pub transcode_jobs: transcode::TranscodeMap,
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
        .unwrap_or(443);
    let tls_cert = std::env::var("PIZERARR_TLS_CERT")
        .unwrap_or_else(|_| "/srv/pizerarr/tls/cert.pem".to_string());
    let tls_key = std::env::var("PIZERARR_TLS_KEY")
        .unwrap_or_else(|_| "/srv/pizerarr/tls/key.pem".to_string());

    tokio::fs::create_dir_all(&media_dir).await?;
    tokio::fs::create_dir_all(&download_dir).await?;

    let session = librqbit::Session::new(download_dir.clone()).await?;
    let library = library::Library::scan(&media_dir, &download_dir).await;

    // Force IPv4 for opensubtitles (resolves to IPv6-only on some networks)
    let http_client = reqwest::Client::builder()
        .local_address(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
        .build()?;

    let state = Arc::new(AppState {
        media_dir,
        download_dir,
        torrent_session: session,
        library: RwLock::new(library),
        torrent_meta: torrent::new_meta_map(),
        http_client,
        subs_cache: subs::new_cache(),
        transcode_jobs: transcode::new_map(),
    });

    let watch_state = state.clone();
    tokio::spawn(async move {
        library::watch_loop(watch_state).await;
    });

    let app = routes::build_router(state);

    // Check if TLS certs exist
    let has_tls = std::path::Path::new(&tls_cert).exists()
        && std::path::Path::new(&tls_key).exists();

    if has_tls {
        // Spawn HTTP->HTTPS redirect on port 80
        tokio::spawn(async move {
            let redirect = axum::Router::new().fallback(get(|req: axum::extract::Request| async move {
                let host = req.headers()
                    .get("host")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("192.168.68.68")
                    .split(':').next().unwrap_or("192.168.68.68");
                let path = req.uri().path_and_query().map(|p| p.as_str()).unwrap_or("/");
                Redirect::permanent(&format!("https://{}{}", host, path))
            }));
            let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 80));
            tracing::info!("HTTP redirect on http://{}", addr);
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            axum::serve(listener, redirect).await.unwrap();
        });

        let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls_cert, &tls_key)
            .await?;

        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!("pizerarr listening on https://{}", addr);

        axum_server::bind_rustls(addr, tls_config)
            .serve(app.into_make_service())
            .await?;
    } else {
        // No TLS — plain HTTP
        let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
        tracing::info!("pizerarr listening on http://{}", addr);
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
    }

    Ok(())
}

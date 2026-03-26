use anyhow::Result;
use librqbit::AddTorrent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TorrentMeta {
    pub title: String,
    pub poster_url: Option<String>,
}

/// Stored alongside AppState — maps torrent ID to user-provided metadata
pub type MetaMap = Arc<RwLock<HashMap<usize, TorrentMeta>>>;

pub fn new_meta_map() -> MetaMap {
    Arc::new(RwLock::new(HashMap::new()))
}

#[derive(Debug, Serialize)]
pub struct TorrentStatus {
    pub id: usize,
    pub name: String,
    pub title: String,
    pub poster_url: Option<String>,
    pub state: String,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub progress_pct: f64,
    pub download_speed: Option<f64>,
    pub finished: bool,
}

pub async fn add_magnet(
    session: &Arc<librqbit::Session>,
    magnet: &str,
    meta: TorrentMeta,
    meta_map: &MetaMap,
) -> Result<usize> {
    let response = session
        .add_torrent(
            AddTorrent::from_url(magnet),
            Some(librqbit::AddTorrentOptions {
                overwrite: true,
                ..Default::default()
            }),
        )
        .await?;

    let id = match response {
        librqbit::AddTorrentResponse::Added(id, _) => id,
        librqbit::AddTorrentResponse::AlreadyManaged(id, _) => id,
        _ => anyhow::bail!("Unexpected add_torrent response"),
    };

    let id_usize: usize = id.into();
    meta_map.write().await.insert(id_usize, meta);

    Ok(id_usize)
}

pub async fn list_torrents(
    session: &Arc<librqbit::Session>,
    meta_map: &MetaMap,
) -> Vec<TorrentStatus> {
    use std::cell::RefCell;

    let meta = meta_map.read().await;
    let statuses = RefCell::new(Vec::new());

    session.with_torrents(|iter| {
        for (id, torrent) in iter {
            let id_usize: usize = id.into();
            let stats = torrent.stats();

            // Use torrent name from the torrent itself, fall back to meta title, then hash
            let torrent_name = torrent.name().unwrap_or_default();
            let default_meta = TorrentMeta::default();
            let m = meta.get(&id_usize).unwrap_or(&default_meta);

            let display_name = if !torrent_name.is_empty() {
                torrent_name
            } else if !m.title.is_empty() {
                m.title.clone()
            } else {
                torrent
                    .info_hash()
                    .0
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>()
            };

            let total = stats.total_bytes;
            let progress = stats.progress_bytes;
            let pct = if total > 0 {
                (progress as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            let download_speed = stats.live.as_ref().map(|l| l.download_speed.mbps);

            statuses.borrow_mut().push(TorrentStatus {
                id: id_usize,
                name: display_name,
                title: m.title.clone(),
                poster_url: m.poster_url.clone(),
                state: format!("{:?}", stats.state),
                progress_bytes: progress,
                total_bytes: total,
                progress_pct: pct,
                download_speed,
                finished: stats.finished,
            });
        }
    });

    statuses.into_inner()
}

pub async fn delete_torrent(
    session: &Arc<librqbit::Session>,
    id: usize,
    meta_map: &MetaMap,
) -> Result<()> {
    session.delete(id.into(), false).await?;
    meta_map.write().await.remove(&id);
    Ok(())
}

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36";
// Use local Python proxy to avoid vendored OpenSSL CA issues
const OS_BASE: &str = "http://127.0.0.1:9191/search/query-";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleResult {
    pub filename: String,
    pub language: String,
    pub download_url: String,
    pub download_path: Option<String>,
    pub format: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OsResult {
    sub_file_name: String,
    sub_language_i_d: String,
    sub_download_link: String,
    sub_format: String,
}

/// In-memory subtitle cache: query -> results
pub type SubsCache = Arc<RwLock<HashMap<String, Vec<SubtitleResult>>>>;

pub fn new_cache() -> SubsCache {
    Arc::new(RwLock::new(HashMap::new()))
}

/// Search OpenSubtitles — checks cache first, fetches if miss
pub async fn search(
    client: &reqwest::Client,
    cache: &SubsCache,
    query: &str,
    lang: &str,
) -> Result<Vec<SubtitleResult>> {
    let key = format!("{}:{}", lang, query.to_lowercase());

    // Check cache
    {
        let c = cache.read().await;
        if let Some(results) = c.get(&key) {
            return Ok(results.clone());
        }
    }

    // Fetch from OpenSubtitles
    let results = fetch_from_api(client, query, lang).await?;

    // Cache it
    cache.write().await.insert(key, results.clone());

    Ok(results)
}

async fn fetch_from_api(
    client: &reqwest::Client,
    query: &str,
    lang: &str,
) -> Result<Vec<SubtitleResult>> {
    let q = query
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let url = format!("{}{}/sublanguageid-{}", OS_BASE, q, lang);

    let resp = client
        .get(&url)
        .header("User-Agent", UA)
        .send()
        .await?;

    let results: Vec<OsResult> = resp.json().await?;

    Ok(results
        .into_iter()
        .take(10)
        .map(|r| SubtitleResult {
            filename: r.sub_file_name,
            language: r.sub_language_i_d,
            download_url: r.sub_download_link,
            download_path: None,
            format: r.sub_format,
        })
        .collect())
}

/// Download a subtitle via the local proxy, save next to video
pub async fn download_to(
    client: &reqwest::Client,
    download_url: &str,
    save_path: &Path,
) -> Result<()> {
    // POST the download URL to the local proxy which fetches + decompresses
    let srt = client
        .post("http://127.0.0.1:9191/download")
        .body(download_url.to_string())
        .send()
        .await?
        .text()
        .await?;

    tokio::fs::write(save_path, &srt).await?;
    Ok(())
}

/// Prefetch subtitles for common/trending titles in background.
/// Called at startup so results are ready when users browse.
pub async fn prefetch_trending(client: &reqwest::Client, cache: &SubsCache, titles: Vec<String>) {
    for title in titles {
        if let Err(e) = search(client, cache, &title, "eng").await {
            tracing::warn!("prefetch subs for '{}': {}", title, e);
        }
        // Be nice to the API
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    tracing::info!("subtitle prefetch complete: {} titles cached", cache.read().await.len());
}

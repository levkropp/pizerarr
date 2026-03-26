use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub magnet: Option<String>,
    pub detail_url: Option<String>,
    pub seeds: u32,
    pub leeches: u32,
    pub size: String,
    pub source: String,
}

// --------------- YTS (movies, JSON API) ---------------

#[derive(Deserialize)]
struct YtsResponse {
    data: Option<YtsData>,
}

#[derive(Deserialize)]
struct YtsData {
    movies: Option<Vec<YtsMovie>>,
}

#[derive(Deserialize)]
struct YtsMovie {
    title_long: String,
    torrents: Vec<YtsTorrent>,
}

#[derive(Deserialize)]
struct YtsTorrent {
    hash: String,
    quality: String,
    size: String,
    seeds: u32,
    peers: u32,
}

pub async fn search_yts(client: &reqwest::Client, query: &str) -> Result<Vec<SearchResult>> {
    let url = format!(
        "https://yts.mx/api/v2/list_movies.json?query_term={}&limit=20&sort_by=seeds",
        urlencoding::encode(query)
    );
    let resp: YtsResponse = client.get(&url).send().await?.json().await?;

    let mut results = Vec::new();
    if let Some(data) = resp.data {
        if let Some(movies) = data.movies {
            for movie in movies {
                for torrent in movie.torrents {
                    let magnet = format!(
                        "magnet:?xt=urn:btih:{}&dn={}&tr=udp://open.stealth.si:80/announce&tr=udp://tracker.opentrackr.org:1337/announce&tr=udp://exodus.desync.com:6969/announce",
                        torrent.hash,
                        urlencoding::encode(&movie.title_long)
                    );
                    results.push(SearchResult {
                        title: format!("{} [{}]", movie.title_long, torrent.quality),
                        magnet: Some(magnet),
                        detail_url: None,
                        seeds: torrent.seeds,
                        leeches: torrent.peers,
                        size: torrent.size.clone(),
                        source: "YTS".to_string(),
                    });
                }
            }
        }
    }
    Ok(results)
}

// --------------- EZTV (TV shows, JSON API) ---------------

#[derive(Deserialize)]
struct EztvResponse {
    torrents: Option<Vec<EztvTorrent>>,
}

#[derive(Deserialize)]
struct EztvTorrent {
    title: String,
    magnet_url: String,
    seeds: u32,
    peers: u32,
    size_bytes: String,
}

fn format_bytes(s: &str) -> String {
    let bytes: u64 = s.parse().unwrap_or(0);
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

pub async fn search_eztv(client: &reqwest::Client, query: &str) -> Result<Vec<SearchResult>> {
    let url = format!(
        "https://eztvx.to/api/get-torrents?limit=20&search={}",
        urlencoding::encode(query)
    );
    let resp: EztvResponse = client.get(&url).send().await?.json().await?;

    let mut results = Vec::new();
    if let Some(torrents) = resp.torrents {
        for t in torrents {
            results.push(SearchResult {
                title: t.title,
                magnet: Some(t.magnet_url),
                detail_url: None,
                seeds: t.seeds,
                leeches: t.peers,
                size: format_bytes(&t.size_bytes),
                source: "EZTV".to_string(),
            });
        }
    }
    Ok(results)
}

// --------------- 1337x (scraping fallback) ---------------

pub async fn search_1337x(client: &reqwest::Client, query: &str) -> Result<Vec<SearchResult>> {
    let url = format!(
        "https://1337x.to/sort-search/{}/seeders/desc/1/",
        urlencoding::encode(query)
    );
    let html = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await?
        .text()
        .await?;

    let doc = scraper::Html::parse_document(&html);
    let row_sel = scraper::Selector::parse(".table-list tbody tr").unwrap();
    let name_sel = scraper::Selector::parse("td.name a:nth-child(2)").unwrap();
    let seeds_sel = scraper::Selector::parse("td.seeds").unwrap();
    let leeches_sel = scraper::Selector::parse("td.leeches").unwrap();
    let size_sel = scraper::Selector::parse("td.size").unwrap();

    let mut results = Vec::new();
    for row in doc.select(&row_sel).take(15) {
        let name_el = match row.select(&name_sel).next() {
            Some(el) => el,
            None => continue,
        };
        let title = name_el.text().collect::<String>().trim().to_string();
        let href = name_el.value().attr("href").unwrap_or_default().to_string();
        let seeds: u32 = row
            .select(&seeds_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().parse().unwrap_or(0))
            .unwrap_or(0);
        let leeches: u32 = row
            .select(&leeches_sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().parse().unwrap_or(0))
            .unwrap_or(0);
        let size = row
            .select(&size_sel)
            .next()
            .map(|e| {
                e.text()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();

        let detail_url = if href.starts_with('/') {
            Some(format!("https://1337x.to{}", href))
        } else {
            None
        };

        results.push(SearchResult {
            title,
            magnet: None, // need to fetch detail page
            detail_url,
            seeds,
            leeches,
            size,
            source: "1337x".to_string(),
        });
    }
    Ok(results)
}

/// Fetch magnet link from a 1337x detail page
pub async fn fetch_1337x_magnet(client: &reqwest::Client, detail_url: &str) -> Result<String> {
    let html = client
        .get(detail_url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await?
        .text()
        .await?;

    let doc = scraper::Html::parse_document(&html);
    let magnet_sel = scraper::Selector::parse("a[href^='magnet:?']").unwrap();

    doc.select(&magnet_sel)
        .next()
        .and_then(|el| el.value().attr("href"))
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("No magnet link found on detail page"))
}

// --------------- PirateBay via apibay.org (JSON API, no key) ---------------

#[derive(Deserialize)]
struct PbResult {
    name: String,
    info_hash: String,
    seeders: String,
    leechers: String,
    size: String,
}

pub async fn search_piratebay(client: &reqwest::Client, query: &str) -> Result<Vec<SearchResult>> {
    let url = format!(
        "https://apibay.org/q.php?q={}",
        urlencoding::encode(query)
    );
    let results: Vec<PbResult> = client.get(&url).send().await?.json().await?;

    Ok(results
        .into_iter()
        .filter(|r| r.name != "No results returned")
        .take(20)
        .map(|r| {
            let magnet = format!(
                "magnet:?xt=urn:btih:{}&dn={}&tr=udp://tracker.opentrackr.org:1337/announce&tr=udp://open.stealth.si:80/announce&tr=udp://exodus.desync.com:6969/announce",
                r.info_hash,
                urlencoding::encode(&r.name)
            );
            SearchResult {
                title: r.name,
                magnet: Some(magnet),
                detail_url: None,
                seeds: r.seeders.parse().unwrap_or(0),
                leeches: r.leechers.parse().unwrap_or(0),
                size: format_bytes(&r.size),
                source: "TPB".to_string(),
            }
        })
        .collect())
}

// --------------- Combined search ---------------

pub async fn search_all(client: &reqwest::Client, query: &str) -> Vec<SearchResult> {
    let (yts, x1337, tpb) = tokio::join!(
        search_yts(client, query),
        search_1337x(client, query),
        search_piratebay(client, query),
    );

    let mut results = Vec::new();
    if let Ok(r) = tpb {
        results.extend(r);
    }
    if let Ok(r) = yts {
        results.extend(r);
    }
    if let Ok(r) = x1337 {
        results.extend(r);
    }

    // Sort by seeds descending
    results.sort_by(|a, b| b.seeds.cmp(&a.seeds));
    results
}

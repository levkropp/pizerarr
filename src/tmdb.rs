use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Unified metadata item used by the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataItem {
    pub id: u64,
    pub title: String,
    pub overview: String,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub year: String,
    pub rating: f64,
    pub media_type: String, // "movie" or "tv"
}

const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36";

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out
}

// ==================== TMDB scraping (no API key) ====================

/// Scrape TMDB search results page — returns movies and TV with posters in one request
pub async fn search_multi(client: &reqwest::Client, query: &str) -> Result<Vec<MetadataItem>> {
    let url = format!(
        "https://www.themoviedb.org/search?query={}",
        urlencoding::encode(query)
    );
    let html = client
        .get(&url)
        .header("User-Agent", UA)
        .send()
        .await?
        .text()
        .await?;

    parse_tmdb_search(&html)
}

fn parse_tmdb_search(html: &str) -> Result<Vec<MetadataItem>> {
    let doc = scraper::Html::parse_document(html);

    // Each search result is in a div.card
    let card_sel = scraper::Selector::parse("div.card").unwrap();
    let title_sel = scraper::Selector::parse("h2").unwrap();
    let link_sel = scraper::Selector::parse("a[href]").unwrap();
    let date_sel = scraper::Selector::parse("span.release_date").unwrap();
    let overview_sel = scraper::Selector::parse("div.overview, p.overview").unwrap();
    let img_sel = scraper::Selector::parse("img[src]").unwrap();

    let mut results = Vec::new();

    for card in doc.select(&card_sel).take(15) {
        // Get title
        let title = match card.select(&title_sel).next() {
            Some(el) => el.text().collect::<String>().trim().to_string(),
            None => continue,
        };

        // Get media type and ID from the link href (e.g., /movie/939243-sonic or /tv/2404-sonic)
        let (media_type, tmdb_id) = match card.select(&link_sel).next() {
            Some(el) => {
                let href = el.value().attr("href").unwrap_or("");
                if let Some(rest) = href.strip_prefix("/movie/") {
                    let id: u64 = rest.split('-').next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    ("movie".to_string(), id)
                } else if let Some(rest) = href.strip_prefix("/tv/") {
                    let id: u64 = rest.split('-').next()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0);
                    ("tv".to_string(), id)
                } else {
                    continue;
                }
            }
            None => continue,
        };

        // Get year from release date
        let year = card
            .select(&date_sel)
            .next()
            .map(|el| {
                let text = el.text().collect::<String>();
                // Extract 4-digit year from date strings like "February 14, 2020"
                text.split(|c: char| !c.is_ascii_digit())
                    .find(|s| s.len() == 4)
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default();

        // Get poster from img src
        let poster_url = card.select(&img_sel).next().and_then(|el| {
            let src = el.value().attr("src").unwrap_or("");
            if src.contains("themoviedb.org") {
                // Upgrade to w342 for better quality
                let upgraded = src
                    .replace("w94_and_h141_face", "w342")
                    .replace("w94_and_h141_bestv2", "w342");
                // Fix CDN domain
                Some(upgraded.replace("media.themoviedb.org/t/p/", "image.tmdb.org/t/p/"))
            } else {
                None
            }
        });

        // Get overview
        let overview = card
            .select(&overview_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        results.push(MetadataItem {
            id: tmdb_id,
            title,
            overview,
            poster_url,
            backdrop_url: None,
            year,
            rating: 0.0,
            media_type,
        });
    }

    Ok(results)
}

// ==================== Trending (cached, refreshed weekly) ====================

#[derive(Serialize, Deserialize)]
struct TrendingCache {
    fetched_at: u64,
    movies: Vec<MetadataItem>,
    shows: Vec<MetadataItem>,
}

const CACHE_MAX_AGE: u64 = 7 * 24 * 3600; // 1 week

fn cache_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("trending_cache.json")
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn load_cache(data_dir: &Path) -> Option<TrendingCache> {
    let path = cache_path(data_dir);
    let data = std::fs::read_to_string(&path).ok()?;
    let cache: TrendingCache = serde_json::from_str(&data).ok()?;
    if now_ts() - cache.fetched_at < CACHE_MAX_AGE {
        Some(cache)
    } else {
        None
    }
}

fn save_cache(data_dir: &Path, cache: &TrendingCache) {
    let path = cache_path(data_dir);
    if let Ok(data) = serde_json::to_string(cache) {
        let _ = std::fs::write(path, data);
    }
}

// --- mdblist trending movies ---

#[derive(Deserialize)]
struct MdblistMovie {
    id: u64,
    title: String,
    release_year: u32,
}

async fn fetch_trending_movies(client: &reqwest::Client) -> Result<Vec<MetadataItem>> {
    let url = "https://mdblist.com/lists/linaspurinis/top-watched-movies-of-the-week/json";
    let movies: Vec<MdblistMovie> = client
        .get(url)
        .header("User-Agent", UA)
        .send()
        .await?
        .json()
        .await?;

    let mut items = Vec::new();
    for movie in movies.into_iter().take(20) {
        let poster = fetch_tmdb_poster(client, movie.id, "movie").await;
        items.push(MetadataItem {
            id: movie.id,
            title: movie.title.clone(),
            overview: String::new(),
            poster_url: poster,
            backdrop_url: None,
            year: movie.release_year.to_string(),
            rating: 0.0,
            media_type: "movie".to_string(),
        });
    }
    Ok(items)
}

/// Scrape a TMDB movie/tv page for the poster image
async fn fetch_tmdb_poster(
    client: &reqwest::Client,
    tmdb_id: u64,
    media_type: &str,
) -> Option<String> {
    let url = format!("https://www.themoviedb.org/{}/{}", media_type, tmdb_id);
    let html = client
        .get(&url)
        .header("User-Agent", UA)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;

    // Extract poster path: /t/p/w500/XXXXX.jpg
    let needle = "/t/p/w500/";
    if let Some(pos) = html.find(needle) {
        let rest = &html[pos + 5..]; // skip "/t/p/" => "w500/XXXXX.jpg..."
        if let Some(end) = rest.find('"').or_else(|| rest.find('\'')) {
            let path = &rest[..end];
            return Some(format!("https://image.tmdb.org/t/p/{}", path));
        }
    }
    None
}

// --- TVmaze top-rated shows ---

#[derive(Deserialize)]
struct TvMazeShow {
    id: u64,
    name: String,
    summary: Option<String>,
    premiered: Option<String>,
    rating: Option<TvMazeRating>,
    image: Option<TvMazeImage>,
}

#[derive(Deserialize)]
struct TvMazeRating {
    average: Option<f64>,
}

#[derive(Deserialize)]
struct TvMazeImage {
    medium: Option<String>,
    original: Option<String>,
}

async fn fetch_trending_shows(client: &reqwest::Client) -> Result<Vec<MetadataItem>> {
    let url = "https://api.tvmaze.com/shows?page=0";
    let shows: Vec<TvMazeShow> = client.get(url).send().await?.json().await?;

    let mut rated: Vec<_> = shows
        .into_iter()
        .filter(|s| {
            s.rating.as_ref().and_then(|r| r.average).unwrap_or(0.0) > 7.0 && s.image.is_some()
        })
        .collect();
    rated.sort_by(|a, b| {
        let ra = b.rating.as_ref().and_then(|r| r.average).unwrap_or(0.0);
        let rb = a.rating.as_ref().and_then(|r| r.average).unwrap_or(0.0);
        ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(rated
        .into_iter()
        .take(20)
        .map(|show| {
            let year = show
                .premiered
                .as_deref()
                .and_then(|d| d.get(..4))
                .unwrap_or("")
                .to_string();
            MetadataItem {
                id: show.id,
                title: show.name,
                overview: show.summary.map(|s| strip_html(&s)).unwrap_or_default(),
                poster_url: show.image.as_ref().and_then(|i| i.medium.clone()),
                backdrop_url: show.image.as_ref().and_then(|i| i.original.clone()),
                year,
                rating: show.rating.and_then(|r| r.average).unwrap_or(0.0),
                media_type: "tv".to_string(),
            }
        })
        .collect())
}

// --- Public API ---

pub async fn get_trending(client: &reqwest::Client, data_dir: &Path) -> Vec<MetadataItem> {
    if let Some(cache) = load_cache(data_dir) {
        tracing::info!("trending: serving from cache");
        let mut items = cache.movies;
        items.extend(cache.shows);
        return items;
    }

    tracing::info!("trending: cache miss, fetching fresh data...");

    let (movies, shows) = tokio::join!(fetch_trending_movies(client), fetch_trending_shows(client));

    let movies = movies.unwrap_or_default();
    let shows = shows.unwrap_or_default();

    save_cache(
        data_dir,
        &TrendingCache {
            fetched_at: now_ts(),
            movies: movies.clone(),
            shows: shows.clone(),
        },
    );

    let mut items = movies;
    items.extend(shows);
    items
}

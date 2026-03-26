# pizerarr

A self-hosted Netflix-like media server in a single 12MB binary. Built for Raspberry Pi, runs anywhere.

**[Website](https://levkropp.github.io/pizerarr)** | **[Download](https://github.com/levkropp/pizerarr/releases/latest)**

## What it does

- **Netflix-like UI** — trending movies/shows with poster art, powered by free APIs (TMDB, TVmaze, mdblist)
- **Torrent search** — searches PirateBay, YTS, and 1337x in parallel
- **Built-in torrent client** — powered by [librqbit](https://github.com/ikatson/rqbit), no qBittorrent needed
- **Subtitles** — search and download from OpenSubtitles, auto-loaded in the video player
- **Storage visualization** — colored usage bar showing what's on your disk
- **Fire TV app** — sideloadable WebView APK included in releases
- **Zero config** — no API keys, no Docker, no databases. Just run the binary.

## Quick start

```bash
# Download (arm64 for Raspberry Pi)
curl -Lo pizerarr https://github.com/levkropp/pizerarr/releases/latest/download/pizerarr-linux-arm64
chmod +x pizerarr

# Run
./pizerarr
```

Open `http://<your-ip>:8080` in a browser.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PIZERARR_PORT` | `8080` | HTTP port |
| `PIZERARR_MEDIA_DIR` | `./media` | Where your video library lives |
| `PIZERARR_DOWNLOAD_DIR` | `./downloads` | Where torrents download to |

### Run as a systemd service

```bash
sudo mv pizerarr /usr/local/bin/
sudo mkdir -p /srv/pizerarr/{media,downloads}

sudo tee /etc/systemd/system/pizerarr.service <<EOF
[Unit]
Description=pizerarr media server
After=network-online.target

[Service]
ExecStart=/usr/local/bin/pizerarr
Environment=PIZERARR_MEDIA_DIR=/srv/pizerarr/media
Environment=PIZERARR_DOWNLOAD_DIR=/srv/pizerarr/downloads
Environment=PIZERARR_PORT=80
AmbientCapabilities=CAP_NET_BIND_SERVICE
Restart=on-failure

[Install]
WantedBy=multi-user.target
EOF

sudo systemctl enable --now pizerarr
```

## Fire TV / Android TV

A 516KB WebView APK is included in every release.

1. Download `pizerarr-firetv.apk` from [Releases](https://github.com/levkropp/pizerarr/releases/latest)
2. Sideload via the **Downloader** app or `adb install pizerarr-firetv.apk`
3. Launch "pizerarr" from your home screen

The APK opens pizerarr fullscreen and works with the D-pad remote. By default it connects to `http://192.168.68.68` — edit `MainActivity.java` to change the IP.

## Building from source

### Server (Rust)

```bash
# Native
cargo build --release

# Cross-compile for Raspberry Pi (arm64)
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu
```

### Fire TV APK

```bash
cd android
gradle wrapper --gradle-version 8.4
./gradlew app:assembleRelease
```

## Architecture

```
pizerarr (single Rust binary)
├── Web server (axum) ─── serves UI + streams video files
├── Torrent engine (librqbit) ─── downloads torrents via BitTorrent
├── Torrent search ─── PirateBay, YTS, 1337x in parallel
├── Metadata ─── TMDB scraping + TVmaze + mdblist (no API keys)
├── Subtitles ─── OpenSubtitles via browser-side fetch
└── Library scanner ─── watches media + download dirs for video files

Frontend: single HTML file + Tailwind CDN + vanilla JS (embedded in binary)
```

## Tech stack

- **Rust** — [axum](https://github.com/tokio-rs/axum) web framework, [librqbit](https://github.com/ikatson/rqbit) torrent engine
- **Frontend** — vanilla JS + Tailwind CSS via CDN, embedded with [rust-embed](https://github.com/pyrossh/rust-embed)
- **Metadata** — TMDB (scraped, no key), TVmaze API, mdblist API, iTunes Search API
- **Subtitles** — OpenSubtitles REST API (fetched browser-side)

## License

MIT

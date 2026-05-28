# BLINDSPOT // MTL

> Navigate Montréal while minimizing exposure to surveillance camera fields of view.

BLINDSPOT maps surveillance cameras across Montréal and routes pedestrians along paths that avoid their vision zones. The goal: prevent trajectory reconstruction from camera footage.

---

## Features

- **Camera map** — OSM-sourced cameras with visualized vision cones (fixed, PTZ/dome, community-added)
- **Privacy routing** — pedestrian routes via OpenRouteService with camera zones as `avoid_polygons`
- **Exposure scoring** — per-segment green/red coloring + % of route under surveillance
- **Live GPS** — real-time position tracking with auto-follow and map-drag pause
- **GPX export** — download route for OsmAnd, Organic Maps, Komoot
- **Route sharing** — shareable URL with A/B points and preset encoded in the hash
- **PWA + offline** — installable, map tiles cached for offline walking
- **Community cameras** — add and classify cameras not yet in OSM

## Stack

| Layer | Technology |
|-------|------------|
| Backend | Rust · Axum 0.7 · SQLite via sqlx |
| Routing | OpenRouteService API (`avoid_polygons`) |
| Camera data | OpenStreetMap via Overpass API |
| Frontend | Vanilla JS · Leaflet 1.9 |
| Deployment | musl static binary · systemd · nginx |

## Setup

### Prerequisites

- Rust (stable toolchain)
- A free [OpenRouteService API key](https://openrouteservice.org/)

### Local development

```bash
git clone https://github.com/Miloud-H/blind-spot.git
cd blind-spot
cp .env.example .env   # add your ORS_API_KEY
cargo run
```

Open [http://localhost:3000](http://localhost:3000). On first launch the app seeds camera data from Overpass — this takes ~30 seconds.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ORS_API_KEY` | *(required)* | OpenRouteService API key |
| `DATABASE_URL` | `sqlite:./blindspot.db` | SQLite path |
| `PORT` | `3000` | HTTP port |

Create a `.env` file at the project root:

```
ORS_API_KEY=your_key_here
DATABASE_URL=sqlite:./blindspot.db
PORT=3000
```

## Deployment

The CI pipeline (`deploy.yml`) builds a static binary for Linux and deploys via SSH:

```bash
cargo build --release --locked --target x86_64-unknown-linux-musl
```

The binary is self-contained (SQLite bundled, no OpenSSL dependency). It runs as a systemd service behind nginx. See `.github/workflows/deploy.yml` for the full pipeline.

## How it works

1. On startup, camera data is imported from [Overpass API](https://overpass-api.de/) (OSM `man_made=surveillance`) into SQLite. The database is refreshed automatically when data is older than 7 days.
2. When a route is requested, the backend queries cameras in the bounding box, builds exclusion polygons (vision cones with a 15 % safety margin), and sends them to ORS as `avoid_polygons`.
3. If ORS returns error 2010 (route blocked by exclusion zones), the backend retries with halved zone radii.
4. The response includes per-segment exposure flags used for green/red route coloring.
5. Route responses are cached in memory for 1 hour (TTL), keyed by start/end/preset.

## License

[AGPL-3.0](LICENSE) — modifications deployed as a network service must be made available as open source.

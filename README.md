# BLINDSPOT // MTL

> Navigate Montréal while minimizing exposure to surveillance camera fields of view.

BLINDSPOT maps surveillance cameras across Montréal and routes pedestrians along paths that avoid their vision zones. The goal: prevent trajectory reconstruction from camera footage.

---

## Features

- **Camera map** — OSM-sourced cameras with visualized vision cones (fixed, PTZ/dome, community-added)
- **Viewshed LOS** — ray-casting against building geometry for realistic cone shapes
- **Avoidance presets** — reduced (×0.5), standard (×1), maximal (×2.2) — updates zone polygons on the map in real time
- **Privacy routing** — pedestrian routes via OpenRouteService with camera zones as `avoid_polygons`
- **Address geocoding** — enter a street address for departure (A) or destination (B), powered by Nominatim
- **Exposure scoring** — per-segment green/red coloring + % of route under surveillance
- **Route history** — last 20 routes saved locally, one-click restore
- **Route sharing** — shareable URL with A/B points and preset encoded in the hash
- **Live GPS** — real-time position tracking with auto-follow and map-drag pause
- **Phone compass** — use device orientation to aim directional cameras when reporting
- **Community cameras** — add, classify and report cameras not yet in OSM
- **Admin panel** — token-protected dashboard at `/admin.html` with camera management, zone visualization, OSM export, and data actions

## Stack

| Layer | Technology |
|-------|------------|
| Backend | Rust · Axum 0.7 · SQLite via sqlx |
| Routing | OpenRouteService API (`avoid_polygons`) |
| Camera data | OpenStreetMap via Overpass API |
| Geocoding | Nominatim (OpenStreetMap, no API key required) |
| Frontend | Vanilla JS (6 modules) · Leaflet 1.9 |
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

Open [http://localhost:3000](http://localhost:3000). On first launch the app seeds camera and building data from Overpass — this takes ~30 seconds.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `ORS_API_KEY` | *(required)* | OpenRouteService API key |
| `ADMIN_TOKEN` | *(empty)* | Token for `/admin.html` — leave empty to disable admin |
| `DATABASE_URL` | `sqlite:./blindspot.db` | SQLite path |
| `PORT` | `3000` | HTTP port |

Create a `.env` file at the project root:

```
ORS_API_KEY=your_key_here
ADMIN_TOKEN=a_secret_token
DATABASE_URL=sqlite:./blindspot.db
PORT=3000
```

## Deployment

The CI pipeline (`deploy.yml`) builds a static binary for Linux and deploys via SSH:

```bash
cargo build --release --locked --target x86_64-unknown-linux-musl
```

The binary is self-contained (SQLite bundled, no OpenSSL dependency). It runs as a systemd service behind nginx. See `.github/workflows/deploy.yml` for the full pipeline.

## Privacy & data model

BLINDSPOT is a privacy tool — its own data practices follow the same principle:

- **No raw IP addresses stored.** On camera creation and reporting, the client IP is hashed with SHA-256 (irreversible) before being written to the database. The original IP is never persisted.
- **Deduplicated reports.** The `camera_reports` table holds `(camera_id, ip_hash)` pairs with a UNIQUE constraint. One hash can report a given camera only once; `report_count` reflects distinct reporters, not click counts.
- **No accounts, no cookies, no tracking.** The frontend uses only `localStorage` for route history and sidebar width.

## How it works

1. On startup, camera and building data is imported from [Overpass API](https://overpass-api.de/) (OSM `man_made=surveillance` + building footprints) into SQLite. Data refreshes automatically when older than 7 days.
2. When a route is requested, the backend queries cameras in the bounding box, runs a viewshed ray-cast against buildings, merges overlapping polygons, and sends them to ORS as `avoid_polygons`.
3. If ORS returns error 2010 (route blocked by exclusion zones), the backend retries with halved zone radii.
4. The response includes per-segment exposure flags used for green/red route coloring.
5. Route responses are cached in memory for 1 hour (TTL, 500 entries max), keyed by start/end/preset.

## Frontend modules

| File | Role |
|------|------|
| `js/app.js` | Leaflet map init, shared globals |
| `js/geo.js` | Pure math — bearing, destination point, haversine |
| `js/viewshed.js` | Building grid + LOS ray-casting |
| `js/cameras.js` | Camera state, rendering, loading, stats |
| `js/routing.js` | Route calculation, geocoding, history, URL hash |
| `js/ui.js` | All event listeners, GPS, compass, boot sequence |

## License

[AGPL-3.0](LICENSE) — modifications deployed as a network service must be made available as open source.

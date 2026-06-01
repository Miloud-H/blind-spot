/// Client Valhalla — routing piéton self-hosted avec exclude_polygons.
/// Compatible avec l'image officielle ghcr.io/valhalla/valhalla:run-latest.
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::models::{LatLng, RouteResult};

// ── Requête ───────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ValhallaRequest {
    locations:        Vec<VLocation>,
    costing:          &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exclude_polygons: Vec<Vec<[f64; 2]>>,
}

#[derive(Serialize)]
struct VLocation {
    lon:       f64,
    lat:       f64,
    #[serde(rename = "type")]
    loc_type:  &'static str,
}

// ── Réponse ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ValhallaResponse {
    trip: VTrip,
}

#[derive(Deserialize)]
struct VTrip {
    legs:    Vec<VLeg>,
    summary: VSummary,
}

#[derive(Deserialize)]
struct VLeg {
    shape: String,   // polyline encodée, précision 1e-6
}

#[derive(Deserialize)]
struct VSummary {
    length: f64,  // km
    time:   f64,  // secondes
}

// ── Erreur Valhalla ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ValhallaError {
    error:      Option<String>,
    error_code: Option<u32>,
}

// ── Appel API ─────────────────────────────────────────────────────────────────

/// Calcule un itinéraire piéton via Valhalla self-hosted.
/// `url` : base URL du service (ex. "http://localhost:8002").
/// `rings` : rings GeoJSON [[lng,lat]…] (fermés) à éviter.
pub async fn get_route(
    client: &Client,
    url:    &str,
    start:  LatLng,
    end:    LatLng,
    rings:  &[Vec<[f64; 2]>],
) -> anyhow::Result<RouteResult> {
    let body = ValhallaRequest {
        locations: vec![
            VLocation { lon: start.lng, lat: start.lat, loc_type: "break" },
            VLocation { lon: end.lng,   lat: end.lat,   loc_type: "break" },
        ],
        costing:          "pedestrian",
        exclude_polygons: rings.to_vec(),
    };

    let route_url = format!("{}/route", url.trim_end_matches('/'));

    tracing::debug!(
        "Valhalla request: {:?} → {:?}, {} zones à éviter",
        start, end, rings.len()
    );

    let http_resp = client
        .post(&route_url)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = http_resp.status();
    if !status.is_success() {
        let raw = http_resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<ValhallaError>(&raw)
            .ok()
            .and_then(|e| e.error.map(|m| {
                if let Some(code) = e.error_code {
                    format!("Valhalla {code} — {m}")
                } else {
                    format!("Valhalla — {m}")
                }
            }))
            .unwrap_or_else(|| format!("HTTP {status} — {raw}"));
        anyhow::bail!(msg);
    }

    let data = http_resp.json::<ValhallaResponse>().await?;
    let leg  = data.trip.legs.into_iter().next()
        .ok_or_else(|| anyhow::anyhow!("Valhalla — réponse sans legs"))?;

    Ok(RouteResult {
        coordinates:  decode_polyline6(&leg.shape),
        distance_m:   data.trip.summary.length * 1000.0,
        duration_sec: data.trip.summary.time,
    })
}

// ── Décodage polyline (Google Polyline Algorithm, précision 1e-6) ─────────────

fn decode_polyline6(encoded: &str) -> Vec<[f64; 2]> {
    let bytes = encoded.as_bytes();
    let mut coords = Vec::new();
    let mut idx = 0usize;
    let mut lat = 0i64;
    let mut lng = 0i64;

    while idx < bytes.len() {
        let (d, ni) = decode_varint(bytes, idx); lat += d; idx = ni;
        let (d, ni) = decode_varint(bytes, idx); lng += d; idx = ni;
        // Valhalla encode lat en premier, GeoJSON veut [lng, lat]
        coords.push([lng as f64 / 1e6, lat as f64 / 1e6]);
    }
    coords
}

fn decode_varint(bytes: &[u8], mut i: usize) -> (i64, usize) {
    let mut result = 0i64;
    let mut shift  = 0u32;
    loop {
        let b = bytes[i] as i64 - 63;
        i += 1;
        result |= (b & 0x1f) << shift;
        shift  += 5;
        if b < 0x20 { break; }
    }
    let result = if result & 1 == 1 { !(result >> 1) } else { result >> 1 };
    (result, i)
}

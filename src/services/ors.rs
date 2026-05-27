/// Client OpenRouteService (ORS) — routing piéton avec avoid_polygons.
use reqwest::Client;
use serde::{Deserialize, Serialize};
use crate::models::LatLng;

// ── Erreur ORS ────────────────────────────────────────────────────────────────

/// Format d'erreur renvoyé par l'API ORS :
/// `{ "error": { "code": 2010, "message": "..." }, "info": {...} }`
/// Parfois aussi `{ "error": "message string" }` (format simplifié).
#[derive(Deserialize)]
#[serde(untagged)]
enum OrsErrorBody {
    Structured { error: OrsErrorDetail },
    Simple { error: String },
}

#[derive(Deserialize)]
struct OrsErrorDetail {
    code: Option<u32>,
    message: String,
}

impl OrsErrorBody {
    fn message(&self) -> String {
        match self {
            OrsErrorBody::Structured { error } => {
                if let Some(code) = error.code {
                    format!("ORS {} — {}", code, error.message)
                } else {
                    format!("ORS — {}", error.message)
                }
            }
            OrsErrorBody::Simple { error } => format!("ORS — {error}"),
        }
    }
}

const ORS_URL: &str = "https://api.openrouteservice.org/v2/directions/foot-walking/geojson";

// ── Requête ───────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct OrsRequest {
    coordinates: Vec<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OrsOptions>,
}

#[derive(Serialize)]
struct OrsOptions {
    avoid_polygons: serde_json::Value,
}

// ── Réponse ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct OrsResponse {
    pub features: Vec<OrsFeature>,
}

#[derive(Deserialize)]
pub struct OrsFeature {
    pub geometry: OrsGeometry,
    pub properties: OrsProperties,
}

#[derive(Deserialize)]
pub struct OrsGeometry {
    /// Coordonnées GeoJSON : [[lng, lat], ...]
    pub coordinates: Vec<[f64; 2]>,
}

#[derive(Deserialize)]
pub struct OrsProperties {
    pub summary: OrsSummary,
}

#[derive(Deserialize)]
pub struct OrsSummary {
    /// Distance en mètres
    pub distance: f64,
    /// Durée en secondes
    pub duration: f64,
}

// ── Appel API ─────────────────────────────────────────────────────────────────

/// Calcule un itinéraire piéton.
/// `rings` : liste de rings GeoJSON [[lng,lat]...] (fermés) à éviter.
pub async fn get_route(
    client: &Client,
    api_key: &str,
    start: LatLng,
    end: LatLng,
    rings: &[Vec<[f64; 2]>],
) -> anyhow::Result<OrsResponse> {
    let body = OrsRequest {
        coordinates: vec![[start.lng, start.lat], [end.lng, end.lat]],
        options: if rings.is_empty() {
            None
        } else {
            Some(OrsOptions {
                avoid_polygons: serde_json::json!({
                    "type": "MultiPolygon",
                    "coordinates": rings.iter().map(|r| vec![r]).collect::<Vec<_>>()
                }),
            })
        },
    };

    tracing::debug!(
        "ORS request: {start:?} → {end:?}, {} zones à éviter",
        rings.len()
    );

    let http_resp = client
        .post(ORS_URL)
        .header("Authorization", api_key)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = http_resp.status();
    if !status.is_success() {
        // Tenter de parser le corps JSON d'erreur ORS pour un message lisible
        let raw = http_resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<OrsErrorBody>(&raw)
            .map(|e| e.message())
            .unwrap_or_else(|_| format!("HTTP {status} — {raw}"));
        anyhow::bail!(msg);
    }

    let resp = http_resp.json::<OrsResponse>().await?;
    Ok(resp)
}

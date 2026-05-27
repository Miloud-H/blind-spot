use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use crate::db;
use sqlx::SqlitePool;

#[derive(Deserialize)]
struct OverpassResponse {
    elements: Vec<OverpassNode>,
}

#[derive(Deserialize)]
struct OverpassNode {
    id: i64,
    lat: f64,
    lon: f64,
    #[serde(default)]
    tags: HashMap<String, String>,
}

// Endpoints publics Overpass (par ordre de préférence).
// Note: overpass-api.de retourne 406 depuis certains réseaux/IPs (block IP).
// kumi.systems est le plus fiable en fallback.
const ENDPOINTS: &[&str] = &[
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass-api.de/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
];

/// Importe les caméras OSM de Montréal dans la base.
/// Utilise POST application/x-www-form-urlencoded — méthode documentée Overpass API.
/// Fallback sur l'endpoint suivant si HTTP ≥ 400 ou erreur réseau.
pub async fn seed_from_overpass(pool: &SqlitePool, client: &Client) -> anyhow::Result<u32> {
    let query = r#"[out:json][timeout:30];node["man_made"="surveillance"](45.45,-73.97,45.70,-73.47);out body;"#;

    tracing::info!("Seed Overpass API...");

    let mut elements: Vec<OverpassNode> = Vec::new();

    for (i, &endpoint) in ENDPOINTS.iter().enumerate() {
        // Petite pause entre les tentatives pour éviter le rate-limit 429
        if i > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        tracing::debug!("Essai {endpoint}");
        let result = client
            .post(endpoint)
            .header("User-Agent", "BlindspotMTL/1.0 (https://github.com/blindspot-mtl)")
            .form(&[("data", query)])    // POST application/x-www-form-urlencoded
            .send()
            .await;

        match result {
            Err(e) => {
                tracing::warn!("{endpoint} injoignable : {e}");
                continue;
            }
            Ok(resp) if !resp.status().is_success() => {
                tracing::warn!("{endpoint} : HTTP {}", resp.status());
                continue;
            }
            Ok(resp) => {
                match resp.json::<OverpassResponse>().await {
                    Err(e) => {
                        tracing::warn!("{endpoint} : réponse invalide : {e}");
                        continue;
                    }
                    Ok(data) => {
                        tracing::info!("{} nœuds reçus depuis {endpoint}", data.elements.len());
                        elements = data.elements;
                        break;
                    }
                }
            }
        }
    }

    if elements.is_empty() {
        anyhow::bail!("Tous les endpoints Overpass ont échoué ou renvoyé 0 résultats");
    }

    let total = elements.len();
    tracing::info!("{total} caméras reçues depuis Overpass");

    let mut ok = 0u32;
    for el in &elements {
        let direction = parse_direction(
            el.tags.get("camera:direction").or_else(|| el.tags.get("direction")),
        );
        let fov = el
            .tags
            .get("camera:angle")
            .or_else(|| el.tags.get("camera:fov"))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(70.0)
            .clamp(10.0, 180.0);

        // Déterminer le type en premier — nécessaire pour le défaut de portée
        let type_raw = el
            .tags
            .get("camera:type")
            .or_else(|| el.tags.get("surveillance:type"))
            .map(String::as_str)
            .unwrap_or("");

        // "panning" = caméra rotative (même comportement que PTZ pour le routing)
        let cam_type = if matches!(type_raw, "ptz" | "dome" | "panoramic" | "panning") {
            "ptz"
        } else {
            "fixed"
        };

        // Portée depuis le tag OSM camera:range, sinon défaut basé sur le type.
        // Ces valeurs correspondent à BASE_RANGE côté frontend (fixed=38, ptz=28).
        let default_range_m: f64 = if cam_type == "ptz" { 28.0 } else { 38.0 };
        let range_m = el
            .tags
            .get("camera:range")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(default_range_m)
            .clamp(5.0, 300.0);

        let name = el
            .tags
            .get("name")
            .or_else(|| el.tags.get("operator"))
            .map(String::as_str);

        let note = el.tags.get("description").map(String::as_str);

        match db::upsert_osm_camera(pool, el.id, el.lat, el.lon, direction, fov, range_m, cam_type, name, note).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::warn!("Erreur insert osm_id={}: {e}", el.id),
        }
    }

    // Enregistrer l'horodatage du seed pour le mécanisme de re-seed automatique (7 jours)
    if ok > 0 {
        if let Err(e) = db::touch_seed_timestamp(pool).await {
            tracing::warn!("Impossible d'enregistrer osm_seeded_at : {e}");
        }
    }

    tracing::info!("Seed terminé : {ok}/{total} caméras upsertées");
    Ok(ok)
}

fn parse_direction(val: Option<&String>) -> Option<f64> {
    let v = val?.trim().to_uppercase();
    match v.as_str() {
        "N"  => Some(0.0),
        "NE" => Some(45.0),
        "E"  => Some(90.0),
        "SE" => Some(135.0),
        "S"  => Some(180.0),
        "SW" => Some(225.0),
        "W"  => Some(270.0),
        "NW" => Some(315.0),
        other => other.parse::<f64>().ok(),
    }
}

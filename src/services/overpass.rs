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
pub async fn seed_from_overpass(
    pool: &SqlitePool,
    client: &Client,
    event_bus: &tokio::sync::broadcast::Sender<String>,
) -> anyhow::Result<u32> {
    // Union de trois schémas de tagging distincts pour les dispositifs de surveillance fixes :
    //   - man_made=surveillance   : caméras de surveillance générales (schéma principal)
    //   - highway=speed_camera    : radars photo
    //   - enforcement=*           : caméras d'application (feux rouges, vitesse moyenne…)
    // Overpass déduplique automatiquement par id au sein de l'union.
    let query = r#"[out:json][timeout:30];
(
  node["man_made"="surveillance"](45.45,-73.97,45.70,-73.47);
  node["highway"="speed_camera"](45.45,-73.97,45.70,-73.47);
  node["enforcement"](45.45,-73.97,45.70,-73.47);
);
out body;"#;

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
        // Déterminer le type en premier — nécessaire pour le défaut de portée et direction
        let type_raw = el
            .tags
            .get("camera:type")
            .or_else(|| el.tags.get("surveillance:type"))
            .map(String::as_str)
            .unwrap_or("");

        // Conserver dome et panoramic comme types distincts (comportements de rendu différents).
        // panning = rotatif simple → même comportement que PTZ.
        let cam_type = match type_raw {
            "ptz" | "panning"  => "ptz",
            "dome"             => "dome",
            "panoramic"        => "panoramic",
            _                  => "fixed",
        };

        // Dôme : coupole opaque → direction réelle inconnue même si le tag est renseigné.
        // PTZ / panoramique : couvrent 360° → direction irrelevante.
        let direction = match cam_type {
            "ptz" | "dome" | "panoramic" => None,
            _ => parse_direction(
                el.tags.get("camera:direction").or_else(|| el.tags.get("direction")),
            ),
        };

        // Portées calibrées selon les données du doc "Pas vue, pas prise" (p.24-31) :
        //   fixed     : ~30 m (identification/reconnaissance, focale ~3 mm Full HD)
        //   dome      : ~20 m (pas de zoom, direction inconnue, couverture réduite)
        //   ptz       : ~50 m (zoom optique ×2.8→12 mm, parfois ×43)
        //   panoramic : ~40 m (multi-capteurs, pas de zoom focal)
        let default_range_m: f64 = match cam_type {
            "ptz"       => 50.0,
            "dome"      => 20.0,
            "panoramic" => 40.0,
            _           => 30.0,
        };
        let range_m = el
            .tags
            .get("camera:range")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(default_range_m)
            .clamp(5.0, 300.0);

        // FOV par défaut basé sur le type (circulaire = 360 ignoré côté rendu, sert juste de marqueur)
        let default_fov: f64 = match cam_type {
            "ptz" | "panoramic" => 360.0,
            "dome"              => 180.0,
            _                   => 80.0,
        };
        let fov = el
            .tags
            .get("camera:angle")
            .or_else(|| el.tags.get("camera:fov"))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(default_fov)
            .clamp(10.0, 360.0);

        // Libellé par défaut pour les radars/caméras d'application — n'ont généralement
        // ni `name` ni `operator` sur OSM contrairement aux caméras man_made=surveillance.
        let enforcement_label = if el.tags.contains_key("enforcement") {
            Some("Radar / caméra d'application de la loi")
        } else if el.tags.get("highway").map(String::as_str) == Some("speed_camera") {
            Some("Radar photo")
        } else {
            None
        };

        let name = el
            .tags
            .get("name")
            .or_else(|| el.tags.get("operator"))
            .map(String::as_str)
            .or(enforcement_label);

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

    // Purge des caméras non revues depuis 3 cycles de reseed (~21j) — probable disparition
    // réelle plutôt qu'un raté ponctuel d'Overpass (endpoint down, timeout...).
    match db::prune_stale_cameras(pool, "osm", 21).await {
        Ok(pruned) if !pruned.is_empty() => {
            tracing::info!("{} caméra(s) OSM obsolète(s) purgée(s) : {:?}", pruned.len(), pruned);
            for id in pruned {
                let _ = event_bus.send(serde_json::to_string(&serde_json::json!({
                    "type": "camera_deleted",
                    "id":   id,
                })).unwrap_or_default());
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Purge des caméras OSM obsolètes échouée : {e}"),
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

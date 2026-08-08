/// Caméras déduites depuis des POI OSM dont la présence de surveillance est garantie.
/// Source = 'inferred' dans la DB — affichage distinct côté frontend.
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use crate::db;
use sqlx::SqlitePool;

// ── Structures Overpass (réutilisées localement) ──────────────────────────────

#[derive(Deserialize)]
struct OverpassResponse {
    elements: Vec<OverpassNode>,
}

#[derive(Deserialize)]
struct OverpassNode {
    id:  i64,
    lat: f64,
    lon: f64,
    #[serde(default)]
    tags: HashMap<String, String>,
}

// ── Définition des types de caméras déduites ──────────────────────────────────

struct InferredType {
    /// Libellé affiché dans le popup
    label:        &'static str,
    /// Filtre Overpass (inséré dans [out:json][timeout:25];<filtre>(bbox);out body;)
    query_filter: &'static str,
    /// Type de caméra : "ptz" pour couverture omnidirectionnelle
    cam_type:     &'static str,
    /// Portée estimée en mètres
    range_m:      f64,
    /// Note affichée dans le popup
    note:         &'static str,
}

/// Types de lieux dont la présence de caméras extérieures est quasi-certaine
/// et dont la couverture outdoor est suffisante pour affecter les itinéraires.
const INFERRED_TYPES: &[InferredType] = &[
    InferredType {
        label:        "Station de métro STM",
        query_filter: r#"node["railway"="station"]["station"="subway"]"#,
        cam_type:     "ptz",
        range_m:      30.0,
        note:         "Caméras STM déduites (surveillance couvre les entrées et abords)",
    },
    InferredType {
        label:        "Poste de police",
        query_filter: r#"node["amenity"="police"]"#,
        cam_type:     "ptz",
        range_m:      40.0,
        note:         "Caméras de sécurité déduites (couverture large autour du bâtiment)",
    },
];

const ENDPOINTS: &[&str] = &[
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass-api.de/api/interpreter",
];

const MTL_BBOX: &str = "45.45,-73.97,45.70,-73.47";

// ── Point d'entrée ────────────────────────────────────────────────────────────

/// Importe les caméras déduites (métro + police) depuis Overpass.
/// Utilise `source='inferred'` pour les distinguer des caméras OSM et communautaires.
pub async fn seed_inferred_cameras(
    pool: &SqlitePool,
    client: &Client,
    event_bus: &tokio::sync::broadcast::Sender<String>,
) -> anyhow::Result<u32> {
    let mut grand_total = 0u32;

    for inf in INFERRED_TYPES {
        tracing::info!("Seed caméras déduites : {}", inf.label);

        let query = format!(
            r#"[out:json][timeout:25];{}({});out body;"#,
            inf.query_filter, MTL_BBOX
        );

        let mut elements: Vec<OverpassNode> = Vec::new();

        for (i, &endpoint) in ENDPOINTS.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            tracing::debug!("Essai {endpoint}");

            let result = client
                .post(endpoint)
                .header("User-Agent", "BlindspotMTL/1.0 (https://github.com/blindspot-mtl)")
                .form(&[("data", query.as_str())])
                .send()
                .await;

            match result {
                Err(e) => { tracing::warn!("{endpoint} injoignable : {e}"); continue; }
                Ok(resp) if !resp.status().is_success() => {
                    tracing::warn!("{endpoint} : HTTP {}", resp.status()); continue;
                }
                Ok(resp) => match resp.json::<OverpassResponse>().await {
                    Err(e) => { tracing::warn!("{endpoint} : réponse invalide : {e}"); continue; }
                    Ok(data) => {
                        tracing::info!("{} {} trouvé(s)", data.elements.len(), inf.label);
                        elements = data.elements;
                        break;
                    }
                },
            }
        }

        if elements.is_empty() {
            tracing::warn!("Aucun résultat pour {} — tous les endpoints ont échoué", inf.label);
            continue;
        }

        let total = elements.len();
        let mut ok = 0u32;

        for el in &elements {
            let name = el.tags.get("name").map(String::as_str).unwrap_or(inf.label);

            let new_cam = db::NewInferredCamera {
                osm_id: el.id, lat: el.lat, lng: el.lon,
                range_m: inf.range_m, cam_type: inf.cam_type,
                name, note: inf.note,
            };
            match db::upsert_inferred_camera(pool, new_cam).await {
                Ok(()) => ok += 1,
                Err(e) => tracing::warn!("Erreur insert inferred osm_id={}: {e}", el.id),
            }
        }

        tracing::info!("Seed {} : {ok}/{total} upsertées", inf.label);
        grand_total += ok;
    }

    // Purge des caméras déduites non revues depuis 3 cycles de reseed (~21j) — le POI source
    // (station, poste de police…) a probablement disparu ou a été dé-taggé sur OSM.
    match db::prune_stale_cameras(pool, "inferred", 21).await {
        Ok(pruned) if !pruned.is_empty() => {
            tracing::info!("{} caméra(s) déduite(s) obsolète(s) purgée(s) : {:?}", pruned.len(), pruned);
            for id in pruned {
                let _ = event_bus.send(serde_json::to_string(&serde_json::json!({
                    "type": "camera_deleted",
                    "id":   id,
                })).unwrap_or_default());
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Purge des caméras déduites obsolètes échouée : {e}"),
    }

    Ok(grand_total)
}

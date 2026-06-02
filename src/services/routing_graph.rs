/// Seed du graphe routier piéton depuis Overpass + calcul des scores d'exposition.
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use crate::{db, geo};
use sqlx::SqlitePool;

const MTL_BBOX: &str = "45.45,-73.97,45.70,-73.47";

const ENDPOINTS: &[&str] = &[
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass-api.de/api/interpreter",
];

// ── Structures Overpass ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct OverpassResponse {
    elements: Vec<OsmWay>,
}

#[derive(Deserialize)]
struct OsmWay {
    nodes:    Vec<i64>,
    geometry: Vec<OsmPoint>,
}

#[derive(Deserialize, Clone, Copy)]
struct OsmPoint {
    lat: f64,
    lon: f64,
}

// ── Seed ──────────────────────────────────────────────────────────────────────

/// Télécharge les rues piétonnes depuis Overpass et construit le graphe routier.
/// Retourne (nœuds insérés, arêtes insérées).
pub async fn seed_routing_graph(pool: &SqlitePool, client: &Client) -> anyhow::Result<(u32, u32)> {
    let query = format!(
        r#"[out:json][timeout:90];
way["highway"~"^(footway|path|pedestrian|steps|living_street|residential|unclassified|tertiary|secondary|primary)$"]["access"!="no"]["access"!="private"]["access"!="customers"]["foot"!="no"]({MTL_BBOX});
out geom;"#
    );

    tracing::info!("Seed graphe routier piéton...");

    let mut ways: Vec<OsmWay> = Vec::new();
    for (i, &endpoint) in ENDPOINTS.iter().enumerate() {
        if i > 0 { tokio::time::sleep(Duration::from_secs(2)).await; }
        tracing::debug!("Essai {endpoint}");

        let result = client
            .post(endpoint)
            .header("User-Agent", "BlindspotMTL/1.0")
            .form(&[("data", query.as_str())])
            .timeout(Duration::from_secs(120))
            .send()
            .await;

        match result {
            Err(e)                                    => { tracing::warn!("{endpoint}: {e}"); continue; }
            Ok(r) if !r.status().is_success()         => { tracing::warn!("{endpoint}: HTTP {}", r.status()); continue; }
            Ok(r) => match r.json::<OverpassResponse>().await {
                Err(e)   => { tracing::warn!("{endpoint}: JSON invalide: {e}"); continue; }
                Ok(data) => {
                    tracing::info!("{} ways reçus depuis {endpoint}", data.elements.len());
                    ways = data.elements;
                    break;
                }
            }
        }
    }

    if ways.is_empty() {
        anyhow::bail!("Aucun way OSM reçu pour le graphe routier");
    }

    // ── Compter les apparitions de chaque nœud (détection des intersections) ──
    let mut node_count: HashMap<i64, usize> = HashMap::new();
    for way in &ways {
        for &nid in &way.nodes {
            *node_count.entry(nid).or_insert(0) += 1;
        }
    }

    // ── Coordonnées par node_id ───────────────────────────────────────────────
    let mut node_coords: HashMap<i64, OsmPoint> = HashMap::new();
    for way in &ways {
        for (i, &nid) in way.nodes.iter().enumerate() {
            node_coords.entry(nid).or_insert(way.geometry[i]);
        }
    }

    // ── Construire les arêtes en splitant les ways aux intersections ──────────
    let mut nodes_ok = 0u32;
    let mut edges_ok = 0u32;

    for way in &ways {
        let n = way.nodes.len();
        if n < 2 { continue; }

        // Insérer le premier nœud (toujours une jonction — endpoint de way)
        if let Some(&pt) = node_coords.get(&way.nodes[0]) {
            if db::upsert_routing_node(pool, way.nodes[0], pt.lat, pt.lon).await.is_ok() {
                nodes_ok += 1;
            }
        }

        let mut seg_start = 0usize;

        for i in 1..n {
            let nid   = way.nodes[i];
            let count = *node_count.get(&nid).unwrap_or(&0);
            let is_junction = i == n - 1 || count > 1;

            if is_junction {
                // Insérer ce nœud de jonction
                if let Some(&pt) = node_coords.get(&nid) {
                    if db::upsert_routing_node(pool, nid, pt.lat, pt.lon).await.is_ok() {
                        nodes_ok += 1;
                    }
                }

                // Distance du segment (somme des distances consécutives)
                let dist: f64 = (seg_start..i).map(|j| {
                    let p1 = way.geometry[j];
                    let p2 = way.geometry[j + 1];
                    geo::haversine_m(p1.lat, p1.lon, p2.lat, p2.lon)
                }).sum();

                if dist >= 0.5 {
                    let from = way.nodes[seg_start];
                    let to   = way.nodes[i];
                    // Bidirectionnel pour la marche à pied
                    if db::insert_routing_edge(pool, from, to, dist).await.is_ok() { edges_ok += 1; }
                    if db::insert_routing_edge(pool, to, from, dist).await.is_ok() { edges_ok += 1; }
                }

                seg_start = i;
            }
        }
    }

    tracing::info!("Graphe routier seedé : {nodes_ok} nœuds, {edges_ok} arêtes");
    Ok((nodes_ok, edges_ok))
}

// ── Calcul des scores d'exposition ───────────────────────────────────────────

/// Calcule et stocke l'exposition caméra de chaque arête pour les 3 presets.
/// Appeler après seed_routing_graph ET après seed des caméras.
pub async fn compute_edge_exposures(pool: &SqlitePool) -> anyhow::Result<u32> {
    let cameras = db::get_all_cameras(pool).await?;
    if cameras.is_empty() {
        tracing::warn!("compute_edge_exposures: aucune caméra en base");
        return Ok(0);
    }

    let edges = db::get_all_routing_edges_with_nodes(pool).await?;
    tracing::info!(
        "Calcul exposition : {} arêtes × {} caméras...",
        edges.len(), cameras.len()
    );

    const STEP_M:       f64 = 5.0;   // 1 point tous les 5m
    const HIGH_MULT:    f64 = 2.2;   // portée max

    let mut updated = 0u32;

    for edge in &edges {
        let cx = (edge.from_lat + edge.to_lat) / 2.0;
        let cy = (edge.from_lng + edge.to_lng) / 2.0;

        // Pré-filtrage spatial : ne tester que les caméras assez proches
        let nearby: Vec<_> = cameras.iter().filter(|cam| {
            let max_r = cam.range_m * HIGH_MULT;
            geo::haversine_m(cx, cy, cam.lat, cam.lng) < max_r + edge.distance_m * 0.5 + 10.0
        }).collect();

        if nearby.is_empty() { continue; }

        // Points d'échantillonnage le long du segment
        let steps = ((edge.distance_m / STEP_M).ceil() as usize).max(2);
        let pts: Vec<(f64, f64)> = (0..=steps).map(|i| {
            let t = i as f64 / steps as f64;
            (
                edge.from_lat + t * (edge.to_lat - edge.from_lat),
                edge.from_lng + t * (edge.to_lng - edge.from_lng),
            )
        }).collect();

        let exp_c = exposure_fraction(&pts, &nearby, 0.5);
        let exp_s = exposure_fraction(&pts, &nearby, 1.0);
        let exp_h = exposure_fraction(&pts, &nearby, HIGH_MULT);

        if exp_c > 0.0 || exp_s > 0.0 || exp_h > 0.0 {
            db::update_edge_exposure(pool, edge.id, exp_c, exp_s, exp_h).await?;
            updated += 1;
        }
    }

    tracing::info!("Exposition calculée : {updated}/{} arêtes exposées", edges.len());
    Ok(updated)
}

fn exposure_fraction(
    pts:      &[(f64, f64)],
    cameras:  &[&crate::models::Camera],
    preset_m: f64,
) -> f64 {
    let exposed = pts.iter()
        .filter(|&&(lat, lng)| {
            cameras.iter().any(|cam| geo::point_in_camera_zone(lat, lng, cam, preset_m))
        })
        .count();
    exposed as f64 / pts.len() as f64
}

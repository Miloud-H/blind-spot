use axum::{extract::State, Json};
use std::sync::atomic::Ordering;
use crate::{
    db, geo,
    error::AppError,
    models::{DirectRoute, LatLng, RouteRequest, RouteResponse, RouteResult},
    route_cache::RouteCache,
    routing::astar::AstarRouter,
    services::ors,
    AppState,
};

/// POST /api/route
///
/// Body: { start, end, avoid_cams?, include_direct? }
///
/// Algorithme :
/// 1. Calcule le bbox autour du trajet + marge dynamique
/// 2. Récupère les caméras dans ce bbox depuis SQLite
/// 3. Génère les rings ORS (cônes / cercles)
/// 4. Appelle ORS avec avoid_polygons
/// 5. Retourne la route GeoJSON + score
pub async fn calculate(
    State(state): State<AppState>,
    Json(req): Json<RouteRequest>,
) -> Result<Json<RouteResponse>, AppError> {
    validate_latlng(req.start.lat, req.start.lng)?;
    validate_latlng(req.end.lat, req.end.lng)?;

    // Sans graphe A* prêt, ORS est requis pour le fallback
    if !state.routing_ready.load(Ordering::Relaxed) && state.config.ors_api_key.is_empty() {
        return Err(AppError::BadRequest(
            "ORS_API_KEY requis tant que le graphe A* n'est pas seedé".into(),
        ));
    }

    // ── Cache hit ────────────────────────────────────────────────────────────
    let cache_key = RouteCache::key(&req);
    if let Some(cached) = state.route_cache.get(&cache_key).await {
        tracing::debug!("Cache HIT ({} entrées)", state.route_cache.len().await);
        return Ok(Json(cached));
    }

    let avoid_cams = req.avoid_cams.unwrap_or(true);
    let include_direct = req.include_direct.unwrap_or(false);

    // ── 1. Bbox + marge dynamique ────────────────────────────────────────────
    let min_lat = f64::min(req.start.lat, req.end.lat);
    let max_lat = f64::max(req.start.lat, req.end.lat);
    let min_lng = f64::min(req.start.lng, req.end.lng);
    let max_lng = f64::max(req.start.lng, req.end.lng);

    let diag = ((max_lat - min_lat).powi(2) + (max_lng - min_lng).powi(2)).sqrt();
    let margin = f64::min(0.05, f64::max(0.012, diag * 0.6));

    // ── 2. Caméras dans le bbox (depuis SQLite) ──────────────────────────────
    // Le backend est la source de vérité : les caméras OSM sont importées au démarrage,
    // les caméras communautaires sont sauvegardées via POST /api/cameras.
    let cameras = if avoid_cams {
        db::get_cameras_in_bbox(
            &state.pool,
            min_lat - margin,
            min_lng - margin,
            max_lat + margin,
            max_lng + margin,
            None,
        )
        .await?
    } else {
        vec![]
    };

    // ── 3. Preset de portée → multiplicateur ─────────────────────────────────
    let preset_mult = match req.range_preset.as_deref() {
        Some("conservative") => 0.5,
        Some("high")         => 2.2,
        _                    => 1.0, // "standard" ou absent
    };

    // ── 4. Cap par proximité à la ligne directe ──────────────────────────────
    // Garde les caméras les plus proches du trajet direct (celles qui gêneront
    // réellement l'itinéraire). Limite de sécurité pour le temps de calcul
    // Valhalla — au-delà de ~300 polygones les performances se dégradent.
    const MAX_ROUTE_POLYGONS: usize = 150;

    let cameras = if cameras.len() > MAX_ROUTE_POLYGONS {
        tracing::debug!(
            "{} caméras dans la zone — cap à {MAX_ROUTE_POLYGONS} (tri par distance à la ligne directe)",
            cameras.len()
        );
        let mut with_dist: Vec<(f64, crate::models::Camera)> = cameras
            .into_iter()
            .map(|cam| {
                let d = geo::dist_to_segment_approx(
                    cam.lat, cam.lng,
                    req.start.lat, req.start.lng,
                    req.end.lat, req.end.lng,
                );
                (d, cam)
            })
            .collect();
        with_dist.sort_unstable_by(|a, b| {
            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        with_dist.into_iter().take(MAX_ROUTE_POLYGONS).map(|(_, c)| c).collect()
    } else {
        cameras
    };

    let cams_count = cameras.len() as u32;
    tracing::debug!(
        "{cams_count} caméras retenues, avoid_cams={avoid_cams}, preset={}",
        req.range_preset.as_deref().unwrap_or("standard")
    );

    let preset = req.range_preset.as_deref().unwrap_or("standard");

    // ── 5. Routeur A* maison (uniquement si graphe complètement seedé) ────────
    let graph_edges = if !state.routing_ready.load(Ordering::Relaxed) {
        vec![] // seed en cours — utiliser le fallback
    } else {
        db::get_routing_edges_in_bbox(
            &state.pool,
            min_lat - margin, min_lng - margin,
            max_lat + margin, max_lng + margin,
            preset,
        ).await.unwrap_or_default()
    };

    if !graph_edges.is_empty() {
        tracing::debug!("{} arêtes chargées pour A*", graph_edges.len());

        let router = AstarRouter::new(&graph_edges, avoid_cams);

        let safe_result = router
            .route(req.start.lat, req.start.lng, req.end.lat, req.end.lng)
            .ok_or_else(|| AppError::External("A* : graphe déconnecté entre ces deux points".into()))?;

        let segments = if avoid_cams {
            geo::compute_segment_exposure(&safe_result.coordinates, &cameras, preset_mult)
        } else {
            vec![]
        };

        tracing::debug!(
            "A* segments exposés : {}/{}",
            segments.iter().filter(|&&e| e).count(), segments.len()
        );

        let direct_route = if include_direct && avoid_cams {
            let direct_router = AstarRouter::new(&graph_edges, false);
            direct_router.route(req.start.lat, req.start.lng, req.end.lat, req.end.lng)
                .map(|dr| DirectRoute {
                    route: serde_json::json!({"type":"LineString","coordinates": dr.coordinates}),
                    distance_km:  dr.distance_m / 1000.0,
                    duration_sec: dr.duration_sec as u32,
                })
        } else {
            None
        };

        let response = RouteResponse {
            route: serde_json::json!({"type":"LineString","coordinates": safe_result.coordinates}),
            distance_km:  safe_result.distance_m / 1000.0,
            duration_sec: safe_result.duration_sec as u32,
            cams_avoided: cams_count,
            relaxed: false,
            segments,
            direct_route,
        };
        state.route_cache.insert(cache_key, response.clone()).await;
        return Ok(Json(response));
    }

    // ── 6. Fallback Valhalla / ORS (graphe non encore seedé) ─────────────────
    tracing::debug!("Graphe routier vide — fallback Valhalla/ORS");

    let buildings = if avoid_cams {
        db::get_buildings_in_bbox(
            &state.pool,
            min_lat - margin, min_lng - margin,
            max_lat + margin, max_lng + margin,
        ).await.unwrap_or_default()
    } else {
        vec![]
    };

    let rings_raw    = geo::cameras_to_ors_rings(&cameras, preset_mult, &buildings);
    let rings_merged = geo::merge_overlapping_rings(rings_raw);
    let start_pt = (req.start.lng, req.start.lat);
    let end_pt   = (req.end.lng,   req.end.lat);
    let (rings, _) = geo::filter_rings_containing_endpoints(rings_merged, start_pt, end_pt);

    const ORS_MARGIN: f64 = 1.15;
    let rings_ors = geo::add_ors_safety_margin(rings.clone(), ORS_MARGIN);

    let (safe_result, relaxed) = {
        match call_router(&state.http_client, &state.config.ors_api_key, req.start, req.end, &rings_ors).await {
            Ok(r) => (r, false),
            Err(e) => {
                let msg = e.to_string();
                if is_no_route_error(&msg) {
                    let rings_half = {
                        let raw    = geo::cameras_to_ors_rings(&cameras, preset_mult * 0.5, &buildings);
                        let merged = geo::merge_overlapping_rings(raw);
                        let (f, _) = geo::filter_rings_containing_endpoints(merged, start_pt, end_pt);
                        geo::add_ors_safety_margin(f, ORS_MARGIN)
                    };
                    let r2 = call_router(&state.http_client, &state.config.ors_api_key, req.start, req.end, &rings_half)
                        .await
                        .map_err(|e2| AppError::External(format!("Routing indisponible après 2 tentatives — {e2}")))?;
                    (r2, true)
                } else {
                    return Err(AppError::External(msg));
                }
            }
        }
    };

    let segments = if avoid_cams {
        geo::compute_segment_exposure(&safe_result.coordinates, &cameras, preset_mult)
    } else {
        vec![]
    };

    let direct_route = if include_direct && avoid_cams {
        match call_router(&state.http_client, &state.config.ors_api_key, req.start, req.end, &[]).await {
            Ok(dr) => Some(DirectRoute {
                route: serde_json::json!({"type":"LineString","coordinates": dr.coordinates}),
                distance_km:  dr.distance_m / 1000.0,
                duration_sec: dr.duration_sec as u32,
            }),
            Err(e) => { tracing::warn!("Route directe indisponible: {e}"); None }
        }
    } else {
        None
    };

    let response = RouteResponse {
        route: serde_json::json!({"type":"LineString","coordinates": safe_result.coordinates}),
        distance_km:  safe_result.distance_m / 1000.0,
        duration_sec: safe_result.duration_sec as u32,
        cams_avoided: cams_count,
        relaxed,
        segments,
        direct_route,
    };
    state.route_cache.insert(cache_key, response.clone()).await;
    Ok(Json(response))
}

fn validate_latlng(lat: f64, lng: f64) -> Result<(), AppError> {
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lng) {
        return Err(AppError::BadRequest(format!(
            "Coordonnées invalides: lat={lat}, lng={lng}"
        )));
    }
    Ok(())
}

async fn call_router(
    client: &reqwest::Client,
    ors_key: &str,
    start:  LatLng,
    end:    LatLng,
    rings:  &[Vec<[f64; 2]>],
) -> anyhow::Result<RouteResult> {
    ors::get_route(client, ors_key, start, end, rings).await
}

fn is_no_route_error(msg: &str) -> bool {
    msg.contains("2010")
}

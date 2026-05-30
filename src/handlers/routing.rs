use axum::{extract::State, Json};
use crate::{
    db, geo,
    error::AppError,
    models::{DirectRoute, RouteRequest, RouteResponse},
    route_cache::RouteCache,
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

    if state.config.ors_api_key.is_empty() {
        return Err(AppError::BadRequest(
            "ORS_API_KEY non configurée — ajouter dans .env".into(),
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

    // ── 4. Cap à MAX_ORS_POLYGONS — tri par proximité à la ligne directe ─────
    // ORS accepte beaucoup de polygones, mais au-delà de ~150 le temps de calcul
    // explose. On garde les caméras les plus proches de la ligne départ→arrivée
    // (heuristique : celles qui gêneront réellement l'itinéraire).
    const MAX_ORS_POLYGONS: usize = 150;

    let cameras = if cameras.len() > MAX_ORS_POLYGONS {
        tracing::debug!(
            "{} caméras dans la zone — cap à {MAX_ORS_POLYGONS} (tri par distance à la ligne directe)",
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
        with_dist.into_iter().take(MAX_ORS_POLYGONS).map(|(_, c)| c).collect()
    } else {
        cameras
    };

    let cams_count = cameras.len() as u32;
    tracing::debug!(
        "{cams_count} caméras retenues, avoid_cams={avoid_cams}, preset={}",
        req.range_preset.as_deref().unwrap_or("standard")
    );

    // ── 5. Bâtiments pour le viewshed LOS ────────────────────────────────────
    // Chargés depuis SQLite (seeded en arrière-plan au démarrage).
    // Vide si seed pas encore terminé → fallback formes simples, transparent.
    let buildings = if avoid_cams {
        db::get_buildings_in_bbox(
            &state.pool,
            min_lat - margin, min_lng - margin,
            max_lat + margin, max_lng + margin,
        )
        .await
        .unwrap_or_default()
    } else {
        vec![]
    };
    tracing::debug!("{} bâtiments chargés pour le viewshed LOS", buildings.len());

    // ── 6. Rings d'exclusion ORS ─────────────────────────────────────────────
    let rings_raw = geo::cameras_to_ors_rings(&cameras, preset_mult, &buildings);

    // ── 7. Fusion des polygones qui se chevauchent ───────────────────────────
    // Réduit le nombre de polygones envoyés à ORS dans les zones denses.
    // Union-Find sur les bboxes + convex hull par cluster.
    let rings_before = rings_raw.len();
    let rings_merged = geo::merge_overlapping_rings(rings_raw);
    tracing::debug!(
        "Rings ORS : {rings_before} → {} après fusion des chevauchements",
        rings_merged.len()
    );

    // ── 7. Filtrage endpoints ─────────────────────────────────────────────────
    // ORS erreur 2010 si start ou end est à l'intérieur d'un avoid_polygon.
    // On retire ces rings (la caméra est trop proche — l'utilisateur l'accepte).
    let start_pt = (req.start.lng, req.start.lat);
    let end_pt   = (req.end.lng,   req.end.lat);
    let (rings, endpoint_removed) =
        geo::filter_rings_containing_endpoints(rings_merged, start_pt, end_pt);
    if endpoint_removed > 0 {
        tracing::info!(
            "{endpoint_removed} ring(s) retirés (contenaient start ou end) — {} restants",
            rings.len()
        );
    }

    // ── 8. Marge de sécurité ORS ─────────────────────────────────────────────
    // Les polygones sont agrandis de 15 % avant envoi à ORS pour compenser la
    // discrétisation (cônes en 10 étapes ≠ arc parfait). L'affichage et le score
    // d'exposition utilisent les rings originaux — seul ORS reçoit la version élargie.
    const ORS_MARGIN: f64 = 1.15;
    let rings_ors = geo::add_ors_safety_margin(rings.clone(), ORS_MARGIN);

    // ── 9. Route sûre — avec retry automatique si ORS 2010 ───────────────────
    // ORS 2010 = "Route could not be found" (zone trop contrainte).
    // Retry avec portée ×0.5 : zones d'exclusion réduites, chemin plus facile à trouver.
    let (safe_resp, relaxed) = {
        match ors::get_route(
            &state.http_client,
            &state.config.ors_api_key,
            req.start,
            req.end,
            &rings_ors,
        )
        .await
        {
            Ok(r) => (r, false),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("2010") {
                    tracing::warn!(
                        "ORS 2010 sur {} rings — retry avec preset×0.5",
                        rings.len()
                    );
                    // Recalculer les rings avec portée réduite + marge
                    let rings_half = {
                        let raw = geo::cameras_to_ors_rings(&cameras, preset_mult * 0.5, &buildings);
                        let merged = geo::merge_overlapping_rings(raw);
                        let (filtered, _) = geo::filter_rings_containing_endpoints(
                            merged, start_pt, end_pt,
                        );
                        geo::add_ors_safety_margin(filtered, ORS_MARGIN)
                    };
                    let r2 = ors::get_route(
                        &state.http_client,
                        &state.config.ors_api_key,
                        req.start,
                        req.end,
                        &rings_half,
                    )
                    .await
                    .map_err(|e2| {
                        AppError::External(format!(
                            "ORS indisponible après 2 tentatives — {e2}"
                        ))
                    })?;
                    tracing::info!(
                        "Retry ORS réussi avec {} rings (portée×0.5)",
                        rings_half.len()
                    );
                    (r2, true)
                } else {
                    return Err(AppError::External(msg));
                }
            }
        }
    };

    let safe_feature = &safe_resp.features[0];

    // ── 10. Exposition par segment ────────────────────────────────────────────
    // Pour chaque segment de la route, on teste si le point passe dans la zone
    // d'une caméra (avec le preset sélectionné). Retourné au frontend pour
    // la coloration verte/rouge de la route et le score d'exposition précis.
    let segments = if avoid_cams {
        geo::compute_segment_exposure(
            &safe_feature.geometry.coordinates,
            &cameras,
            preset_mult,
        )
    } else {
        vec![]
    };

    tracing::debug!(
        "Segments exposés : {}/{} ({:.0}%)",
        segments.iter().filter(|&&e| e).count(),
        segments.len(),
        if segments.is_empty() { 0.0 } else {
            segments.iter().filter(|&&e| e).count() as f64 / segments.len() as f64 * 100.0
        }
    );

    // ── 9. Route directe (optionnelle) ───────────────────────────────────────
    let direct_route = if include_direct && avoid_cams {
        match ors::get_route(
            &state.http_client,
            &state.config.ors_api_key,
            req.start,
            req.end,
            &[], // sans exclusions
        )
        .await
        {
            Ok(direct_resp) => {
                let df = &direct_resp.features[0];
                Some(DirectRoute {
                    route: serde_json::json!({
                        "type": "LineString",
                        "coordinates": df.geometry.coordinates
                    }),
                    distance_km: df.properties.summary.distance / 1000.0,
                    duration_sec: df.properties.summary.duration as u32,
                })
            }
            Err(e) => {
                tracing::warn!("Route directe indisponible: {e}");
                None
            }
        }
    } else {
        None
    };

    let response = RouteResponse {
        route: serde_json::json!({
            "type": "LineString",
            "coordinates": safe_feature.geometry.coordinates
        }),
        distance_km: safe_feature.properties.summary.distance / 1000.0,
        duration_sec: safe_feature.properties.summary.duration as u32,
        cams_avoided: cams_count,
        relaxed,
        segments,
        direct_route,
    };

    // ── Cache store ──────────────────────────────────────────────────────────
    state.route_cache.insert(cache_key, response.clone()).await;
    tracing::debug!("Cache MISS → stocké ({} entrées)", state.route_cache.len().await);

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

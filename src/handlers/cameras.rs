use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use std::sync::atomic::Ordering;
use crate::{
    db, rate_limit,
    error::AppError,
    models::{BboxQuery, Camera, CreateCameraRequest},
    services::routing_graph,
    AppState,
};

/// GET /api/cameras?bbox=minLat,minLng,maxLat,maxLng
pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<BboxQuery>,
) -> Result<Json<Vec<Camera>>, AppError> {
    let (min_lat, min_lng, max_lat, max_lng) = parse_bbox(params.bbox.as_deref())?;
    let source = params.source.as_deref();
    let cameras = db::get_cameras_in_bbox(&state.pool, min_lat, min_lng, max_lat, max_lng, source).await?;
    Ok(Json(cameras))
}

/// POST /api/cameras
pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateCameraRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    if req.lat < -90.0 || req.lat > 90.0 || req.lng < -180.0 || req.lng > 180.0 {
        return Err(AppError::BadRequest("Coordonnées invalides".into()));
    }

    let ip_hash = rate_limit::hash_ip(&headers);
    let outcome = db::insert_camera(&state.pool, &req, &ip_hash).await?;
    let id = outcome.id;

    // Doublon quasi-certain : la caméra existante a été corroborée, rien de nouveau
    // n'a été créé — pas d'évènement "camera_added" ni de recalcul d'exposition.
    if outcome.merged {
        return Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "id": id,
                "merged": true,
                "message": "Caméra déjà connue à cet endroit — signalement pris en compte"
            })),
        ));
    }

    let _ = state.event_bus.send(serde_json::to_string(&serde_json::json!({
        "type": "camera_added",
        "camera": {
            "id":        id,
            "lat":       req.lat,
            "lng":       req.lng,
            "direction": req.direction,
            "fov":       req.fov.unwrap_or(70.0),
            "range_m":   req.range_m.unwrap_or(30.0),
            "cam_type":  req.cam_type.as_deref().unwrap_or("unknown"),
            "name":      req.name,
            "source":    "user",
            "note":      req.note,
        }
    })).unwrap_or_default());

    // Mettre à jour les expositions du graphe A* autour de la nouvelle caméra
    if state.routing_ready.load(Ordering::Relaxed) {
        let pool_bg = state.pool.clone();
        let cam = Camera {
            id,
            osm_id:    None,
            lat:       req.lat,
            lng:       req.lng,
            direction: req.direction,
            fov:       req.fov.unwrap_or(70.0),
            range_m:   req.range_m.unwrap_or(30.0),
            cam_type:  req.cam_type.unwrap_or_else(|| "unknown".into()),
            name:      req.name,
            operator:  None,
            note:      req.note,
            source:    "user".into(),
            verified:  false,
        };
        tokio::spawn(async move {
            match routing_graph::recompute_exposures_near_camera(&pool_bg, &cam).await {
                Ok(n)  => tracing::debug!("Graphe mis à jour : {n} arêtes recalculées"),
                Err(e) => tracing::warn!("Recalcul exposition échoué : {e}"),
            }
        });
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "message": "Caméra ajoutée" })),
    ))
}


/// Nombre de signalements indépendants ("SIGNALER COMME RETIRÉE") au-delà duquel une
/// caméra est supprimée automatiquement, sans revue admin — 3 hash IP distincts requis,
/// assez robuste contre un signalement isolé/malveillant tout en évitant le goulot manuel.
const AUTO_REMOVE_REPORT_THRESHOLD: i64 = 3;

/// POST /api/cameras/:id/report — signalement dédupliqué par hash IP.
/// Au-delà du seuil, la caméra est retirée automatiquement (corroborée par plusieurs sources).
pub async fn report(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Vérifie que la caméra existe
    let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await?;
    if exists == 0 {
        return Err(AppError::NotFound);
    }

    let ip_hash = rate_limit::hash_ip(&headers);

    // INSERT OR IGNORE : si le hash a déjà signalé cette caméra, aucun effet
    let inserted = sqlx::query(
        "INSERT OR IGNORE INTO camera_reports (camera_id, ip_hash) VALUES (?, ?)",
    )
    .bind(id)
    .bind(&ip_hash)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if inserted == 0 {
        return Ok(Json(serde_json::json!({ "message": "Déjà signalé" })));
    }

    let report_count: i64 = sqlx::query_scalar(
        "UPDATE cameras SET report_count = report_count + 1 WHERE id = ? RETURNING report_count",
    )
    .bind(id)
    .fetch_one(&state.pool)
    .await?;

    if report_count >= AUTO_REMOVE_REPORT_THRESHOLD {
        db::delete_camera(&state.pool, id).await?;

        let _ = state.event_bus.send(serde_json::to_string(&serde_json::json!({
            "type": "camera_deleted",
            "id":   id,
        })).unwrap_or_default());

        return Ok(Json(serde_json::json!({
            "message": "Caméra retirée — confirmée absente par plusieurs signalements",
            "removed": true
        })));
    }

    Ok(Json(serde_json::json!({ "message": "Signalement enregistré" })))
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parse "minLat,minLng,maxLat,maxLng" → (f64, f64, f64, f64)
/// Défaut : bounding box Montréal complet
fn parse_bbox(bbox: Option<&str>) -> Result<(f64, f64, f64, f64), AppError> {
    let s = bbox.unwrap_or("45.45,-73.97,45.70,-73.47");
    let parts: Vec<f64> = s
        .split(',')
        .map(|v| v.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .map_err(|_| AppError::BadRequest("Format bbox invalide. Attendu: minLat,minLng,maxLat,maxLng".into()))?;

    if parts.len() != 4 {
        return Err(AppError::BadRequest("Le bbox doit contenir exactement 4 valeurs".into()));
    }
    Ok((parts[0], parts[1], parts[2], parts[3]))
}

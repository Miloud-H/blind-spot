use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use crate::{
    db,
    error::AppError,
    models::{BboxQuery, Camera, CreateCameraRequest},
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
    Json(req): Json<CreateCameraRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    // Validation basique
    if req.lat < -90.0 || req.lat > 90.0 || req.lng < -180.0 || req.lng > 180.0 {
        return Err(AppError::BadRequest("Coordonnées invalides".into()));
    }

    let id: i64 = db::insert_camera(&state.pool, &req).await?;

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

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "message": "Caméra ajoutée" })),
    ))
}


/// POST /api/cameras/:id/report — incrémente le compteur de signalements
pub async fn report(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let affected = sqlx::query(
        "UPDATE cameras SET report_count = report_count + 1 WHERE id = ?",
    )
    .bind(id)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(AppError::NotFound);
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

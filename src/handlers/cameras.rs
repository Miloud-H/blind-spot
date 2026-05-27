use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use crate::{
    db,
    error::AppError,
    models::{BboxQuery, Camera, CreateCameraRequest},
    services::overpass,
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
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "id": id, "message": "Caméra ajoutée" })),
    ))
}

/// POST /api/admin/seed — importe les caméras OSM depuis Overpass
pub async fn seed(State(state): State<AppState>) -> Result<Json<serde_json::Value>, AppError> {
    let count = overpass::seed_from_overpass(&state.pool, &state.http_client)
        .await
        .map_err(|e| AppError::External(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "seeded": count,
        "message": format!("{count} caméras importées depuis OSM")
    })))
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

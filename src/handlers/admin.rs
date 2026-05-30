use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use crate::{
    error::AppError,
    models::ReportedCamera,
    services::overpass,
    AppState,
};

fn require_admin(headers: &HeaderMap, token: &str) -> Result<(), AppError> {
    if token.is_empty() {
        return Err(AppError::BadRequest("ADMIN_TOKEN non configuré sur le serveur".into()));
    }
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != format!("Bearer {token}") {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

/// GET /api/admin/reports — caméras avec au moins 1 signalement
pub async fn list_reports(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ReportedCamera>>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let cameras = sqlx::query_as::<_, ReportedCamera>(
        "SELECT id, lat, lng, cam_type, source, report_count, name
         FROM cameras WHERE report_count > 0
         ORDER BY report_count DESC LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(cameras))
}

/// DELETE /api/admin/cameras/:id — supprime une caméra
pub async fn delete_camera(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let affected = sqlx::query("DELETE FROM cameras WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(AppError::NotFound);
    }
    Ok((StatusCode::OK, Json(serde_json::json!({ "deleted": id }))))
}

/// POST /api/admin/reseed — déclenche un re-import OSM complet
pub async fn reseed(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let n = overpass::seed_from_overpass(&state.pool, &state.http_client)
        .await
        .map_err(|e| AppError::External(e.to_string()))?;

    Ok(Json(serde_json::json!({
        "seeded": n,
        "message": format!("{n} caméras importées depuis OSM")
    })))
}

/// GET /api/admin/stats — statistiques globales
pub async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras")
        .fetch_one(&state.pool).await?;
    let osm: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'osm'")
        .fetch_one(&state.pool).await?;
    let user: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'user'")
        .fetch_one(&state.pool).await?;
    let reported: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE report_count > 0")
        .fetch_one(&state.pool).await?;
    let cache_size = state.route_cache.len().await as i64;

    Ok(Json(serde_json::json!({
        "cameras_total":    total,
        "cameras_osm":      osm,
        "cameras_user":     user,
        "cameras_reported": reported,
        "route_cache_size": cache_size,
    })))
}

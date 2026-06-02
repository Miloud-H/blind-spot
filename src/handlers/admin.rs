use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    Json,
};
use std::sync::atomic::Ordering;
use crate::{
    db,
    error::AppError,
    models::{AdminCamerasQuery, BulkDeleteRequest, Camera, ReportedCamera, UpdateCameraRequest},
    services::{overpass, routing_graph},
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

/// GET /api/admin/stats — statistiques globales enrichies
pub async fn stats(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let total: i64    = sqlx::query_scalar("SELECT COUNT(*) FROM cameras").fetch_one(&state.pool).await?;
    let osm: i64      = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'osm'").fetch_one(&state.pool).await?;
    let user: i64     = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'user'").fetch_one(&state.pool).await?;
    let inferred: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'inferred'").fetch_one(&state.pool).await?;
    let reported: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE report_count > 0").fetch_one(&state.pool).await?;
    let user_reported: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'user' AND report_count > 0").fetch_one(&state.pool).await?;
    let type_fixed: i64   = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE cam_type = 'fixed'").fetch_one(&state.pool).await?;
    let type_ptz: i64     = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE cam_type = 'ptz'").fetch_one(&state.pool).await?;
    let type_unknown: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE cam_type = 'unknown'").fetch_one(&state.pool).await?;
    let with_direction: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE direction IS NOT NULL").fetch_one(&state.pool).await?;
    let cache_size = state.route_cache.len().await as i64;

    Ok(Json(serde_json::json!({
        "cameras_total":      total,
        "cameras_osm":        osm,
        "cameras_user":       user,
        "cameras_inferred":   inferred,
        "cameras_reported":   reported,
        "cameras_user_reported": user_reported,
        "type_fixed":         type_fixed,
        "type_ptz":           type_ptz,
        "type_unknown":       type_unknown,
        "cameras_with_direction": with_direction,
        "route_cache_size":   cache_size,
    })))
}

/// GET /api/admin/reports — caméras avec au moins 1 signalement
pub async fn list_reports(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ReportedCamera>>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let cameras = sqlx::query_as::<_, ReportedCamera>(
        "SELECT id, lat, lng, cam_type, source, report_count, name, direction
         FROM cameras WHERE report_count > 0
         ORDER BY report_count DESC LIMIT 200",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(cameras))
}

/// GET /api/admin/cameras?page=1&limit=50&source=&cam_type=&reported=
pub async fn list_cameras(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AdminCamerasQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let page  = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * limit;

    let mut wheres: Vec<String> = Vec::new();

    if let Some(ref src) = params.source {
        if !["osm", "user", "inferred"].contains(&src.as_str()) {
            return Err(AppError::BadRequest("source invalide".into()));
        }
        wheres.push(format!("source = '{src}'"));
    }
    if let Some(ref t) = params.cam_type {
        if !["fixed", "ptz", "unknown"].contains(&t.as_str()) {
            return Err(AppError::BadRequest("cam_type invalide".into()));
        }
        wheres.push(format!("cam_type = '{t}'"));
    }
    if params.reported.unwrap_or(false) {
        wheres.push("report_count > 0".to_string());
    }

    let where_sql = if wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wheres.join(" AND "))
    };

    let total: i64 = sqlx::query_scalar(
        &format!("SELECT COUNT(*) FROM cameras {where_sql}")
    ).fetch_one(&state.pool).await?;

    let cameras = sqlx::query_as::<_, ReportedCamera>(
        &format!(
            "SELECT id, lat, lng, cam_type, source, report_count, name, direction \
             FROM cameras {where_sql} ORDER BY id DESC LIMIT {limit} OFFSET {offset}"
        )
    ).fetch_all(&state.pool).await?;

    Ok(Json(serde_json::json!({
        "cameras": cameras,
        "total":   total,
        "page":    page,
        "limit":   limit,
        "pages":   ((total as f64) / (limit as f64)).ceil() as i64,
    })))
}

/// DELETE /api/admin/cameras — supprime une liste de caméras en un seul appel
pub async fn delete_cameras_bulk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BulkDeleteRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    if body.ids.is_empty() {
        return Ok(Json(serde_json::json!({ "deleted": 0 })));
    }

    let placeholders = body.ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!("DELETE FROM cameras WHERE id IN ({placeholders})");

    let mut q = sqlx::query(&sql);
    for id in &body.ids {
        q = q.bind(id);
    }

    let affected = q.execute(&state.pool).await?.rows_affected();

    for id in &body.ids {
        let _ = state.event_bus.send(serde_json::to_string(&serde_json::json!({
            "type": "camera_deleted",
            "id":   id,
        })).unwrap_or_default());
    }

    Ok(Json(serde_json::json!({ "deleted": affected })))
}

/// PATCH /api/admin/cameras/:id — déplace / modifie une caméra
pub async fn update_camera(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(body): Json<UpdateCameraRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    if !(-90.0..=90.0).contains(&body.lat) || !(-180.0..=180.0).contains(&body.lng) {
        return Err(AppError::BadRequest("Coordonnées invalides".into()));
    }
    if !["fixed", "ptz", "unknown"].contains(&body.cam_type.as_str()) {
        return Err(AppError::BadRequest("cam_type invalide".into()));
    }

    let affected = sqlx::query(
        "UPDATE cameras SET lat=?, lng=?, direction=?, fov=?, range_m=?, cam_type=?, name=?, note=? \
         WHERE id=?"
    )
    .bind(body.lat)
    .bind(body.lng)
    .bind(body.direction)
    .bind(body.fov)
    .bind(body.range_m)
    .bind(&body.cam_type)
    .bind(body.name.as_deref())
    .bind(body.note.as_deref())
    .bind(id)
    .execute(&state.pool)
    .await?
    .rows_affected();

    if affected == 0 {
        return Err(AppError::NotFound);
    }

    let _ = state.event_bus.send(serde_json::to_string(&serde_json::json!({
        "type":      "camera_updated",
        "id":        id,
        "lat":       body.lat,
        "lng":       body.lng,
        "direction": body.direction,
        "fov":       body.fov,
        "range_m":   body.range_m,
        "cam_type":  body.cam_type,
        "name":      body.name,
        "note":      body.note,
    })).unwrap_or_default());

    Ok(Json(serde_json::json!({ "updated": id })))
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

    let _ = state.event_bus.send(serde_json::to_string(&serde_json::json!({
        "type": "camera_deleted",
        "id":   id,
    })).unwrap_or_default());

    Ok((StatusCode::OK, Json(serde_json::json!({ "deleted": id }))))
}

/// GET /api/admin/export/osm — exporte les caméras communautaires en .osm (JOSM)
pub async fn export_osm(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    let cameras = sqlx::query_as::<_, Camera>(
        "SELECT id, osm_id, lat, lng, direction, fov, range_m, cam_type, name, \
                operator, note, source, verified \
         FROM cameras WHERE source = 'user' AND report_count = 0 ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await?;

    let mut nodes = String::new();
    for (i, cam) in cameras.iter().enumerate() {
        let node_id = -(i as i64 + 1);

        let type_tag = match cam.cam_type.as_str() {
            "ptz"   => "    <tag k=\"camera:type\" v=\"dome\"/>\n",
            "fixed" => "    <tag k=\"camera:type\" v=\"fixed\"/>\n",
            _       => "",
        };
        let dir_tag = cam.direction
            .map(|d| format!("    <tag k=\"direction\" v=\"{:.0}\"/>\n", d))
            .unwrap_or_default();
        let note_tag = cam.note.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("    <tag k=\"note\" v=\"{}\"/>\n", s.replace('"', "&quot;")))
            .unwrap_or_default();
        let name_tag = cam.name.as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("    <tag k=\"name\" v=\"{}\"/>\n", s.replace('"', "&quot;")))
            .unwrap_or_default();

        nodes.push_str(&format!(
            "  <node id=\"{node_id}\" lat=\"{:.7}\" lon=\"{:.7}\" version=\"1\" action=\"create\">\n\
             {type_tag}{dir_tag}{name_tag}{note_tag}\
             \t<tag k=\"man_made\" v=\"surveillance\"/>\n\
             \t<tag k=\"surveillance\" v=\"outdoor\"/>\n\
             \t<tag k=\"surveillance:type\" v=\"camera\"/>\n\
             \t<tag k=\"source\" v=\"survey\"/>\n\
             \t<tag k=\"note:source\" v=\"BlindSpot MTL\"/>\n\
           </node>\n",
            cam.lat, cam.lng,
        ));
    }

    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <osm version=\"0.6\" generator=\"BlindSpot MTL\">\n\
         {nodes}</osm>\n"
    );

    let response = axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, HeaderValue::from_static("application/xml; charset=utf-8"))
        .header(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"blindspot-export.osm\""))
        .body(Body::from(xml))
        .map_err(|e| AppError::External(e.to_string()))?;

    Ok(response)
}

/// DELETE /api/admin/cache — vide le cache de routes en mémoire
pub async fn clear_cache(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;
    state.route_cache.clear().await;
    Ok(Json(serde_json::json!({ "message": "Cache vidé" })))
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

/// POST /api/admin/reseed-graph — recrée le graphe routier depuis zéro en arrière-plan
pub async fn reseed_graph(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    require_admin(&headers, &state.config.admin_token)?;

    // Marque le graphe comme non-prêt immédiatement
    state.routing_ready.store(false, Ordering::Relaxed);
    let _ = db::set_metadata(&state.pool, "routing_graph_ready", "0").await;

    // Vide les tables
    sqlx::query("DELETE FROM routing_edges").execute(&state.pool).await
        .map_err(|e| AppError::External(e.to_string()))?;
    sqlx::query("DELETE FROM routing_nodes").execute(&state.pool).await
        .map_err(|e| AppError::External(e.to_string()))?;

    // Lance le seed en arrière-plan
    let pool_bg   = state.pool.clone();
    let client_bg = state.http_client.clone();
    let ready_flag = state.routing_ready.clone();
    tokio::spawn(async move {
        match routing_graph::seed_routing_graph(&pool_bg, &client_bg).await {
            Err(e) => { tracing::warn!("Seed graphe routier échoué : {e}"); return; }
            Ok((n, e)) => tracing::info!("Graphe routier seedé : {n} nœuds, {e} arêtes"),
        }
        match routing_graph::compute_edge_exposures(&pool_bg).await {
            Err(e) => { tracing::warn!("Calcul exposition échoué : {e}"); return; }
            Ok(u)  => tracing::info!("Exposition calculée : {u} arêtes exposées"),
        }
        let _ = db::set_metadata(&pool_bg, "routing_graph_ready", "1").await;
        ready_flag.store(true, Ordering::Relaxed);
        tracing::info!("Routeur A* prêt ✓");
    });

    Ok(Json(serde_json::json!({
        "message": "Re-seed du graphe routier lancé en arrière-plan (~2h)"
    })))
}

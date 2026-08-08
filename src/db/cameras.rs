use sqlx::SqlitePool;
use crate::models::{Camera, CreateCameraRequest};

// ── Lecture ──────────────────────────────────────────────────────────────────

/// Retourne les caméras dans un bounding box (lat/lng REAL — pas de PostGIS).
/// `source` optionnel : 'osm' | 'user' | None = toutes.
pub async fn get_cameras_in_bbox(
    pool: &SqlitePool,
    min_lat: f64,
    min_lng: f64,
    max_lat: f64,
    max_lng: f64,
    source: Option<&str>,
) -> sqlx::Result<Vec<Camera>> {
    // La clause source est injectée comme chaîne SQL statique (valeurs contrôlées côté Rust).
    let source_clause = match source {
        Some(s) if s == "osm"  => " AND source = 'osm'",
        Some(s) if s == "user" => " AND source = 'user'",
        _                       => "",
    };

    let sql = format!(
        r#"
        SELECT
            id, osm_id, lat, lng, direction, fov, range_m, cam_type,
            name, operator, note, source,
            CAST(verified AS BOOLEAN) AS verified
        FROM cameras
        WHERE lat BETWEEN $1 AND $2
          AND lng BETWEEN $3 AND $4
          {source_clause}
        ORDER BY id
        LIMIT 5000
        "#
    );

    sqlx::query_as::<_, Camera>(&sql)
        .bind(min_lat)
        .bind(max_lat)
        .bind(min_lng)
        .bind(max_lng)
        .fetch_all(pool)
        .await
}

/// Toutes les caméras (pour le calcul d'exposition des arêtes).
pub async fn get_all_cameras(pool: &SqlitePool) -> sqlx::Result<Vec<Camera>> {
    sqlx::query_as::<_, Camera>(
        "SELECT id, osm_id, lat, lng, direction, fov, range_m, cam_type,
                name, operator, note, source,
                CAST(verified AS BOOLEAN) AS verified
         FROM cameras",
    )
    .fetch_all(pool)
    .await
}

// ── Écriture ─────────────────────────────────────────────────────────────────

/// Insère une caméra communautaire (source = 'user').
/// Retourne last_insert_rowid.
pub async fn insert_camera(
    pool: &SqlitePool,
    req: &CreateCameraRequest,
    created_from: &str,
) -> sqlx::Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO cameras (lat, lng, direction, fov, range_m, cam_type, name, note, source, created_from)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'user', $9)
        "#,
    )
    .bind(req.lat)
    .bind(req.lng)
    .bind(req.direction)
    .bind(req.fov.unwrap_or(70.0))
    .bind(req.range_m.unwrap_or(30.0))
    .bind(req.cam_type.as_deref().unwrap_or("unknown"))
    .bind(req.name.as_deref())
    .bind(req.note.as_deref())
    .bind(created_from)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Upsert d'une caméra OSM (par osm_id). Appelé par le seed Overpass.
/// `range_m` est extrait du tag `camera:range` ou vaut 30 m par défaut.
pub async fn upsert_osm_camera(
    pool: &SqlitePool,
    osm_id: i64,
    lat: f64,
    lng: f64,
    direction: Option<f64>,
    fov: f64,
    range_m: f64,
    cam_type: &str,
    name: Option<&str>,
    note: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO cameras (osm_id, lat, lng, direction, fov, range_m, cam_type, name, note, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'osm')
        ON CONFLICT(osm_id) DO UPDATE SET
            lat       = excluded.lat,
            lng       = excluded.lng,
            direction = excluded.direction,
            fov       = excluded.fov,
            range_m   = excluded.range_m,
            cam_type  = excluded.cam_type,
            name      = excluded.name,
            note      = excluded.note
        "#,
    )
    .bind(osm_id)
    .bind(lat)
    .bind(lng)
    .bind(direction)
    .bind(fov)
    .bind(range_m)
    .bind(cam_type)
    .bind(name)
    .bind(note)
    .execute(pool)
    .await?;

    Ok(())
}

/// Upsert d'une caméra déduite (source = 'inferred').
/// Identifiée par osm_id du POI source (métro, police…).
pub async fn upsert_inferred_camera(
    pool: &SqlitePool,
    osm_id: i64,
    lat: f64,
    lng: f64,
    range_m: f64,
    cam_type: &str,
    name: &str,
    note: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO cameras (osm_id, lat, lng, direction, fov, range_m, cam_type, name, note, source)
        VALUES ($1, $2, $3, NULL, 70.0, $4, $5, $6, $7, 'inferred')
        ON CONFLICT(osm_id) DO UPDATE SET
            lat      = excluded.lat,
            lng      = excluded.lng,
            range_m  = excluded.range_m,
            cam_type = excluded.cam_type,
            name     = excluded.name,
            note     = excluded.note
        "#,
    )
    .bind(osm_id)
    .bind(lat)
    .bind(lng)
    .bind(range_m)
    .bind(cam_type)
    .bind(name)
    .bind(note)
    .execute(pool)
    .await?;

    Ok(())
}

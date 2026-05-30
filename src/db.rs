use sqlx::SqlitePool;
use crate::models::{BuildingGeom, Camera, CreateCameraRequest};

// ── Métadonnées ───────────────────────────────────────────────────────────────

/// Lit une valeur depuis la table `metadata`. Retourne `None` si la clé n'existe pas.
#[allow(dead_code)]
pub async fn get_metadata(pool: &SqlitePool, key: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT value FROM metadata WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Écrit (ou met à jour) une valeur dans `metadata`.
#[allow(dead_code)]
pub async fn set_metadata(pool: &SqlitePool, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO metadata (key, value, updated_at)
        VALUES ($1, $2, datetime('now'))
        ON CONFLICT(key) DO UPDATE SET
            value      = excluded.value,
            updated_at = datetime('now')
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Enregistre l'horodatage du dernier import OSM (valeur = datetime SQLite).
/// Utilisé par le mécanisme de re-seed automatique.
pub async fn touch_seed_timestamp(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO metadata (key, value, updated_at)
        VALUES ('osm_seeded_at', datetime('now'), datetime('now'))
        ON CONFLICT(key) DO UPDATE SET
            value      = datetime('now'),
            updated_at = datetime('now')
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Retourne le nombre de jours entiers depuis le dernier import OSM.
/// Retourne `i64::MAX` si aucun import n'a encore eu lieu.
pub async fn days_since_osm_seed(pool: &SqlitePool) -> i64 {
    let result: sqlx::Result<Option<Option<f64>>> = sqlx::query_scalar(
        "SELECT julianday('now') - julianday(value) FROM metadata WHERE key = 'osm_seeded_at'",
    )
    .fetch_optional(pool)
    .await;

    result
        .ok()
        .flatten()
        .flatten()
        .map(|d| d as i64)
        .unwrap_or(i64::MAX)
}

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

// ── Bâtiments ────────────────────────────────────────────────────────────────

/// Retourne les bâtiments dont la bbox chevauche la zone [min_lat..max_lat, min_lng..max_lng].
pub async fn get_buildings_in_bbox(
    pool: &SqlitePool,
    min_lat: f64, min_lng: f64,
    max_lat: f64, max_lng: f64,
) -> anyhow::Result<Vec<BuildingGeom>> {
    // buildings dont la bbox intersecte le rectangle :
    //   bld.min_lat <= query_max_lat  et  bld.max_lat >= query_min_lat
    //   bld.min_lng <= query_max_lng  et  bld.max_lng >= query_min_lng
    let rows: Vec<(f64, f64, f64, f64, String)> = sqlx::query_as(
        "SELECT min_lat, max_lat, min_lng, max_lng, geom \
         FROM buildings \
         WHERE min_lat <= ? AND max_lat >= ? \
           AND min_lng <= ? AND max_lng >= ? \
         LIMIT 60000"
    )
    .bind(max_lat).bind(min_lat)
    .bind(max_lng).bind(min_lng)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for (min_lat, max_lat, min_lng, max_lng, geom_str) in rows {
        let pts: Vec<[f64; 2]> = serde_json::from_str(&geom_str)?;
        out.push(BuildingGeom { pts, min_lat, max_lat, min_lng, max_lng });
    }
    Ok(out)
}

pub async fn count_buildings(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM buildings")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

// ── Écriture ─────────────────────────────────────────────────────────────────

/// Insère une caméra communautaire (source = 'user').
/// Retourne last_insert_rowid.
pub async fn insert_camera(pool: &SqlitePool, req: &CreateCameraRequest) -> sqlx::Result<i64> {
    let result = sqlx::query(
        r#"
        INSERT INTO cameras (lat, lng, direction, fov, range_m, cam_type, name, note, source)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'user')
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

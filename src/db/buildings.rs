use sqlx::SqlitePool;
use crate::models::BuildingGeom;

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

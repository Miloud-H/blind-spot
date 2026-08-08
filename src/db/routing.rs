use sqlx::SqlitePool;
use crate::models::GraphEdge;

// ── Graphe routier ────────────────────────────────────────────────────────────

pub async fn count_routing_edges(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM routing_edges")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

pub async fn upsert_routing_node(pool: &SqlitePool, id: i64, lat: f64, lng: f64) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO routing_nodes (id, lat, lng) VALUES (?, ?, ?)
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(id).bind(lat).bind(lng)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_routing_edge(
    pool: &SqlitePool,
    from_node: i64,
    to_node: i64,
    distance_m: f64,
) -> sqlx::Result<i64> {
    let r = sqlx::query(
        "INSERT INTO routing_edges (from_node, to_node, distance_m) VALUES (?, ?, ?)",
    )
    .bind(from_node).bind(to_node).bind(distance_m)
    .execute(pool)
    .await?;
    Ok(r.last_insert_rowid())
}

pub async fn update_edge_exposure(
    pool: &SqlitePool,
    id: i64,
    exp_conserv: f64,
    exp_standard: f64,
    exp_high: f64,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE routing_edges
         SET exp_conserv  = MAX(exp_conserv,  ?),
             exp_standard = MAX(exp_standard, ?),
             exp_high     = MAX(exp_high,     ?)
         WHERE id = ?",
    )
    .bind(exp_conserv).bind(exp_standard).bind(exp_high).bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remet à zéro toutes les expositions (avant un recalcul complet).
pub async fn reset_edge_exposures(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query("UPDATE routing_edges SET exp_conserv = 0, exp_standard = 0, exp_high = 0")
        .execute(pool)
        .await?;
    Ok(())
}

/// Retourne les arêtes dans un bbox avec coordonnées des nœuds (pour calcul exposition local).
pub async fn get_routing_edges_with_nodes_in_bbox(
    pool: &SqlitePool,
    min_lat: f64, min_lng: f64,
    max_lat: f64, max_lng: f64,
) -> sqlx::Result<Vec<GraphEdge>> {
    sqlx::query_as::<_, GraphEdge>(
        "SELECT e.id, e.from_node, e.to_node, e.distance_m,
                n1.lat AS from_lat, n1.lng AS from_lng,
                n2.lat AS to_lat,   n2.lng AS to_lng
         FROM routing_edges e
         JOIN routing_nodes n1 ON e.from_node = n1.id
         JOIN routing_nodes n2 ON e.to_node   = n2.id
         WHERE n1.lat BETWEEN ? AND ? AND n1.lng BETWEEN ? AND ?",
    )
    .bind(min_lat).bind(max_lat).bind(min_lng).bind(max_lng)
    .fetch_all(pool)
    .await
}

/// Retourne toutes les arêtes avec coordonnées des nœuds (pour calcul exposition).
pub async fn get_all_routing_edges_with_nodes(pool: &SqlitePool) -> sqlx::Result<Vec<GraphEdge>> {
    sqlx::query_as::<_, GraphEdge>(
        "SELECT e.id, e.from_node, e.to_node, e.distance_m,
                n1.lat AS from_lat, n1.lng AS from_lng,
                n2.lat AS to_lat,   n2.lng AS to_lng
         FROM routing_edges e
         JOIN routing_nodes n1 ON e.from_node = n1.id
         JOIN routing_nodes n2 ON e.to_node   = n2.id",
    )
    .fetch_all(pool)
    .await
}

/// Retourne les arêtes dans un bbox avec l'exposition du preset sélectionné.
/// Retourne aussi les coordonnées des nœuds pour construire le graphe A*.
pub async fn get_routing_edges_in_bbox(
    pool: &SqlitePool,
    min_lat: f64, min_lng: f64,
    max_lat: f64, max_lng: f64,
    preset: &str,
) -> sqlx::Result<Vec<GraphEdge>> {
    let exp_col = match preset {
        "conservative" => "e.exp_conserv",
        "high"         => "e.exp_high",
        _              => "e.exp_standard",
    };
    let sql = format!(
        "SELECT e.id, e.from_node, e.to_node, e.distance_m,
                n1.lat AS from_lat, n1.lng AS from_lng,
                n2.lat AS to_lat,   n2.lng AS to_lng,
                {exp_col} AS exposure
         FROM routing_edges e
         JOIN routing_nodes n1 ON e.from_node = n1.id
         JOIN routing_nodes n2 ON e.to_node   = n2.id
         WHERE n1.lat BETWEEN ? AND ? AND n1.lng BETWEEN ? AND ?"
    );
    sqlx::query_as::<_, GraphEdge>(&sql)
        .bind(min_lat).bind(max_lat)
        .bind(min_lng).bind(max_lng)
        .fetch_all(pool)
        .await
}

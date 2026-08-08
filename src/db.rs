use sqlx::SqlitePool;
use crate::geo;
use crate::models::{BuildingGeom, Camera, CreateCameraRequest, GraphEdge};

// ── Déduplication cross-source ───────────────────────────────────────────────
//
// Deux caméras ne sont considérées comme "le même appareil physique" que si elles
// sont proches ET du même type. La direction ne départage que pour 'fixed' (seul
// type où elle est fiable) — un écart trop grand ou une direction inconnue rend le
// verdict ambigu plutôt que de fusionner à l'aveugle (perte de données potentielle).

const DUP_RADIUS_M: f64 = 8.0;
const DUP_DIRECTION_TOLERANCE_DEG: f64 = 40.0;

/// Résultat d'une recherche de doublon potentiel.
#[derive(Clone, Copy)]
pub struct DuplicateMatch {
    pub id: i64,
    /// true = même type + direction compatible (ou type sans direction fiable) → quasi-certain.
    /// false = même type + proche mais direction incompatible/inconnue → ambigu, à signaler.
    pub confident: bool,
}

#[derive(sqlx::FromRow)]
struct CandidateRow {
    id: i64,
    lat: f64,
    lng: f64,
    direction: Option<f64>,
}

/// Écart angulaire absolu entre deux azimuts (0-360°), en tenant compte du wraparound.
fn angular_diff_deg(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// Cherche une caméra existante susceptible d'être le même appareil physique que
/// (lat, lng, cam_type, direction). Pré-filtre par bounding box en SQL, confirme par
/// Haversine en Rust. `exclude_id` permet d'ignorer la caméra elle-même lors d'un upsert.
/// `source_filter` restreint la recherche à une source donnée (ex: 'user' lors d'un
/// upsert OSM, pour ne comparer qu'aux contributions communautaires).
pub async fn find_duplicate_candidate(
    pool: &SqlitePool,
    lat: f64,
    lng: f64,
    cam_type: &str,
    direction: Option<f64>,
    exclude_id: Option<i64>,
    source_filter: Option<&str>,
) -> sqlx::Result<Option<DuplicateMatch>> {
    // Bbox large (marge) autour du rayon de dédup — filtrage grossier avant Haversine exact.
    let deg_lat = DUP_RADIUS_M / 111_000.0 * 1.5;
    let deg_lng = deg_lat / lat.to_radians().cos().max(0.2);

    let rows: Vec<CandidateRow> = sqlx::query_as(
        r#"
        SELECT id, lat, lng, direction FROM cameras
        WHERE cam_type = $1
          AND lat BETWEEN $2 AND $3
          AND lng BETWEEN $4 AND $5
          AND ($6 IS NULL OR id != $6)
          AND ($7 IS NULL OR source = $7)
        "#,
    )
    .bind(cam_type)
    .bind(lat - deg_lat).bind(lat + deg_lat)
    .bind(lng - deg_lng).bind(lng + deg_lng)
    .bind(exclude_id)
    .bind(source_filter)
    .fetch_all(pool)
    .await?;

    // Types sans direction fiable (couverture 360° ou coupole opaque) : proximité + type suffisent
    // à évoquer un doublon, mais jamais avec certitude — toujours ambigu.
    let type_has_direction = cam_type == "fixed";

    let mut best: Option<DuplicateMatch> = None;
    for row in rows {
        let dist = geo::haversine_m(lat, lng, row.lat, row.lng);
        if dist > DUP_RADIUS_M {
            continue;
        }

        let confident = if type_has_direction {
            match (direction, row.direction) {
                (Some(a), Some(b)) => angular_diff_deg(a, b) <= DUP_DIRECTION_TOLERANCE_DEG,
                _ => false, // direction manquante d'un côté → ambigu, pas de fusion silencieuse
            }
        } else {
            false // dome/ptz/panoramic : jamais de fusion automatique, toujours revue admin
        };

        // Un match confiant prime sur un match ambigu déjà trouvé.
        if best.is_none() || (confident && !best.as_ref().unwrap().confident) {
            best = Some(DuplicateMatch { id: row.id, confident });
        }
    }

    Ok(best)
}

/// Marque une caméra comme corroborée par une source indépendante (relève `verified`).
pub async fn corroborate_camera(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE cameras SET verified = 1 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Flag non-bloquant : signale qu'une caméra pourrait être un doublon d'une autre,
/// sans fusionner ni supprimer — laisse la décision à une revue admin.
pub async fn flag_possible_duplicate(pool: &SqlitePool, id: i64, of_id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE cameras SET possible_duplicate_of = ? WHERE id = ?")
        .bind(of_id)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Efface le flag de doublon (revue admin : "ce n'est pas un doublon, garder les deux").
/// Retourne `true` si une ligne a été affectée.
pub async fn dismiss_possible_duplicate(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let affected = sqlx::query("UPDATE cameras SET possible_duplicate_of = NULL WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// Supprime une caméra par id. Retourne `true` si une ligne a été supprimée.
pub async fn delete_camera(pool: &SqlitePool, id: i64) -> sqlx::Result<bool> {
    let affected = sqlx::query("DELETE FROM cameras WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(affected > 0)
}

/// Purge les caméras `source` non revues par un reseed depuis `grace_days` jours
/// (probable disparition réelle plutôt qu'un raté ponctuel d'Overpass).
/// Retourne les id supprimés, pour diffusion d'évènements `camera_deleted`.
pub async fn prune_stale_cameras(
    pool: &SqlitePool,
    source: &str,
    grace_days: i64,
) -> sqlx::Result<Vec<i64>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM cameras
         WHERE source = $1
           AND last_seen_at IS NOT NULL
           AND julianday('now') - julianday(last_seen_at) > $2",
    )
    .bind(source)
    .bind(grace_days as f64)
    .fetch_all(pool)
    .await?;

    if !ids.is_empty() {
        sqlx::query("DELETE FROM cameras WHERE source = ? AND last_seen_at IS NOT NULL AND julianday('now') - julianday(last_seen_at) > ?")
            .bind(source)
            .bind(grace_days as f64)
            .execute(pool)
            .await?;
    }

    Ok(ids)
}

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

/// Résultat de `insert_camera` : soit une nouvelle ligne créée, soit une caméra
/// existante corroborée (doublon quasi-certain — rien de nouveau n'a été inséré).
pub struct InsertOutcome {
    pub id: i64,
    pub merged: bool,
}

/// Insère une caméra communautaire (source = 'user'), avec dédup :
/// - doublon quasi-certain (même type, même zone, direction compatible) → pas d'insertion,
///   la caméra existante est marquée corroborée (`verified`) et son id est retourné.
/// - doublon ambigu (même type, même zone, direction inconnue/incompatible) → insertion,
///   mais `possible_duplicate_of` est renseigné pour une revue admin.
pub async fn insert_camera(
    pool: &SqlitePool,
    req: &CreateCameraRequest,
    created_from: &str,
) -> sqlx::Result<InsertOutcome> {
    let cam_type = req.cam_type.as_deref().unwrap_or("unknown");
    let dup = find_duplicate_candidate(pool, req.lat, req.lng, cam_type, req.direction, None, None).await?;

    if let Some(DuplicateMatch { id, confident: true }) = dup {
        corroborate_camera(pool, id).await?;
        return Ok(InsertOutcome { id, merged: true });
    }

    let id = insert_camera_row(pool, req, created_from).await?;
    if let Some(DuplicateMatch { id: of_id, confident: false }) = dup {
        flag_possible_duplicate(pool, id, of_id).await?;
    }
    Ok(InsertOutcome { id, merged: false })
}

async fn insert_camera_row(
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

/// Après upsert d'une caméra 'osm'/'inferred', compare aux caméras communautaires
/// à proximité : un doublon quasi-certain (même type + direction compatible) est résolu
/// en supprimant l'entrée communautaire désormais redondante (l'officielle fait foi) et en
/// corroborant la nouvelle ligne ; un doublon ambigu est simplement flaggé pour revue admin.
async fn resolve_cross_source_duplicate(
    pool: &SqlitePool,
    new_id: i64,
    lat: f64,
    lng: f64,
    cam_type: &str,
    direction: Option<f64>,
) -> sqlx::Result<()> {
    let dup = find_duplicate_candidate(pool, lat, lng, cam_type, direction, None, Some("user")).await?;
    match dup {
        Some(DuplicateMatch { id, confident: true }) => {
            delete_camera(pool, id).await?;
            corroborate_camera(pool, new_id).await?;
        }
        Some(DuplicateMatch { id, confident: false }) => {
            flag_possible_duplicate(pool, id, new_id).await?;
        }
        None => {}
    }
    Ok(())
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
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO cameras (osm_id, lat, lng, direction, fov, range_m, cam_type, name, note, source, last_seen_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'osm', datetime('now'))
        ON CONFLICT(osm_id) DO UPDATE SET
            lat          = excluded.lat,
            lng          = excluded.lng,
            direction    = excluded.direction,
            fov          = excluded.fov,
            range_m      = excluded.range_m,
            cam_type     = excluded.cam_type,
            name         = excluded.name,
            note         = excluded.note,
            last_seen_at = datetime('now')
        RETURNING id
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
    .fetch_one(pool)
    .await?;

    resolve_cross_source_duplicate(pool, id, lat, lng, cam_type, direction).await?;

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
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO cameras (osm_id, lat, lng, direction, fov, range_m, cam_type, name, note, source, last_seen_at)
        VALUES ($1, $2, $3, NULL, 70.0, $4, $5, $6, $7, 'inferred', datetime('now'))
        ON CONFLICT(osm_id) DO UPDATE SET
            lat          = excluded.lat,
            lng          = excluded.lng,
            range_m      = excluded.range_m,
            cam_type     = excluded.cam_type,
            name         = excluded.name,
            note         = excluded.note,
            last_seen_at = datetime('now')
        RETURNING id
        "#,
    )
    .bind(osm_id)
    .bind(lat)
    .bind(lng)
    .bind(range_m)
    .bind(cam_type)
    .bind(name)
    .bind(note)
    .fetch_one(pool)
    .await?;

    resolve_cross_source_duplicate(pool, id, lat, lng, cam_type, None).await?;

    Ok(())
}

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

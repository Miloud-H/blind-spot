use serde::{Deserialize, Serialize};

// ── Caméra (depuis DB) ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Camera {
    pub id: i64,   // SQLite INTEGER (AUTOINCREMENT → i64)
    pub osm_id: Option<i64>,
    pub lat: f64,
    pub lng: f64,
    pub direction: Option<f64>,
    pub fov: f64,
    pub range_m: f64,
    pub cam_type: String,
    pub name: Option<String>,
    pub operator: Option<String>,
    pub note: Option<String>,
    pub source: String,
    pub verified: bool,
}

// ── Requêtes HTTP ────────────────────────────────────────────────────────────

/// ?bbox=minLat,minLng,maxLat,maxLng&source=user|osm
#[derive(Debug, Deserialize)]
pub struct BboxQuery {
    pub bbox:   Option<String>,
    pub source: Option<String>, // 'osm' | 'user' | absent = toutes
}

/// POST /api/cameras
#[derive(Debug, Deserialize)]
pub struct CreateCameraRequest {
    pub lat: f64,
    pub lng: f64,
    pub direction: Option<f64>,
    pub fov: Option<f64>,
    pub range_m: Option<f64>,
    pub cam_type: Option<String>,
    pub name: Option<String>,
    pub note: Option<String>,
}

/// POST /api/route
#[derive(Debug, Deserialize)]
pub struct RouteRequest {
    pub start: LatLng,
    pub end: LatLng,
    /// Éviter les champs de vision (défaut: true)
    pub avoid_cams: Option<bool>,
    /// Renvoyer aussi la route directe pour comparaison (défaut: false)
    pub include_direct: Option<bool>,
    /// Agressivité des zones d'évitement : "conservative" | "standard" | "high" (défaut: "standard")
    pub range_preset: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

// ── Bâtiments (viewshed LOS) ─────────────────────────────────────────────────

/// Polygone de bâtiment chargé depuis SQLite pour le ray-casting.
/// `pts` : [[lat, lng], …] — ordre Leaflet (lat en premier).
/// `min/max` : bbox précalculée pour le filtrage spatial rapide.
#[derive(Debug)]
pub struct BuildingGeom {
    pub pts:     Vec<[f64; 2]>,
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lng: f64,
    pub max_lng: f64,
}

// ── Admin ────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ReportedCamera {
    pub id:           i64,
    pub lat:          f64,
    pub lng:          f64,
    pub cam_type:     String,
    pub source:       String,
    pub report_count: i64,
    pub name:         Option<String>,
    pub direction:    Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct AdminCamerasQuery {
    pub page:     Option<i64>,
    pub limit:    Option<i64>,
    pub source:   Option<String>,
    pub cam_type: Option<String>,
    pub reported: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct BulkDeleteRequest {
    pub ids: Vec<i64>,
}

/// PATCH /api/admin/cameras/:id — mise à jour d'une caméra communautaire
#[derive(Debug, Deserialize)]
pub struct UpdateCameraRequest {
    pub lat:       f64,
    pub lng:       f64,
    pub direction: Option<f64>,
    pub fov:       f64,
    pub range_m:   f64,
    pub cam_type:  String,
    pub name:      Option<String>,
    pub note:      Option<String>,
}

/// GET /api/admin/zones — zones d'évitement fusionnées pour la vue courante
#[derive(Debug, Deserialize)]
pub struct ZonesQuery {
    pub bbox:   Option<String>,
    pub preset: Option<String>,
}

// ── Graphe routier ────────────────────────────────────────────────────────────

/// Arête du graphe routier avec coordonnées des nœuds et exposition caméra.
/// Retourné par `db::get_routing_edges_in_bbox` et `get_all_routing_edges_with_nodes`.
#[derive(Debug, sqlx::FromRow)]
pub struct GraphEdge {
    pub id:          i64,
    pub from_node:   i64,
    pub to_node:     i64,
    pub distance_m:  f64,
    pub from_lat:    f64,
    pub from_lng:    f64,
    pub to_lat:      f64,
    pub to_lng:      f64,
    /// Exposition pour le preset sélectionné (0.0–1.0). NULL → 0.0 via default.
    #[sqlx(default)]
    pub exposure:    f64,
}

// ── Résultat de routing (commun ORS + Valhalla) ──────────────────────────────

/// Type de retour partagé entre les clients ORS et Valhalla.
pub struct RouteResult {
    /// Coordonnées GeoJSON [[lng, lat], …]
    pub coordinates:  Vec<[f64; 2]>,
    /// Distance en mètres
    pub distance_m:   f64,
    /// Durée en secondes
    pub duration_sec: f64,
}

// ── Réponses HTTP ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct RouteResponse {
    /// GeoJSON LineString de la route sûre
    pub route: serde_json::Value,
    pub distance_km: f64,
    pub duration_sec: u32,
    pub cams_avoided: u32,
    /// true si ORS a échoué (2010) et que la portée a été réduite de moitié au retry
    pub relaxed: bool,
    /// Exposition par segment : Vec<bool>, longueur = coords.len()-1.
    /// true = ce segment passe dans au moins une zone de caméra.
    pub segments: Vec<bool>,
    /// Route directe (sans avoidance) si demandée
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_route: Option<DirectRoute>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DirectRoute {
    pub route: serde_json::Value,
    pub distance_km: f64,
    pub duration_sec: u32,
}

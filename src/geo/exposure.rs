//! Détermine si un point/segment de route tombe dans la zone de surveillance d'une caméra.

use crate::models::Camera;
use super::math::{bearing_deg, haversine_m};

// ── Exposition par segment ────────────────────────────────────────────────────

/// Teste si un point (lat, lng) tombe dans la zone de surveillance d'une caméra.
/// Miroir exact de `isPointInCameraZone` côté JavaScript (cone + cercle PTZ).
pub fn point_in_camera_zone(lat: f64, lng: f64, cam: &Camera, preset_mult: f64) -> bool {
    let range = cam.range_m * preset_mult;
    let dist  = haversine_m(lat, lng, cam.lat, cam.lng);
    if dist > range * 1.15 { return false; } // rejet rapide

    let is_circular = matches!(cam.cam_type.as_str(), "ptz" | "dome" | "panoramic");
    if is_circular || cam.direction.is_none() {
        return dist <= range;
    }
    // Caméra directionnelle : vérifier le cône
    let dir = cam.direction.unwrap();
    let bearing = bearing_deg(cam.lat, cam.lng, lat, lng);
    let mut diff = (bearing - dir).abs();
    if diff > 180.0 { diff = 360.0 - diff; }
    diff <= cam.fov / 2.0 && dist <= range
}

/// Retourne `true` si le segment GeoJSON [coord1, coord2] (format [lng, lat])
/// passe dans la zone de surveillance d'au moins une caméra.
///
/// Vérifie 3 points : les deux extrémités + le milieu.
/// Suffisant pour les segments ORS urbains courts (< 50 m en général).
pub fn segment_in_camera_zone(
    coord1: [f64; 2],
    coord2: [f64; 2],
    cameras: &[Camera],
    preset_mult: f64,
) -> bool {
    let check_pts = [
        (coord1[1], coord1[0]),                                              // lat, lng
        (coord2[1], coord2[0]),
        ((coord1[1] + coord2[1]) / 2.0, (coord1[0] + coord2[0]) / 2.0),    // milieu
    ];
    cameras.iter().any(|cam| {
        check_pts
            .iter()
            .any(|&(lat, lng)| point_in_camera_zone(lat, lng, cam, preset_mult))
    })
}

/// Calcule l'exposition de chaque segment d'une route GeoJSON.
/// Retourne un `Vec<bool>` de longueur `coords.len() - 1`.
pub fn compute_segment_exposure(
    coords: &[[f64; 2]],
    cameras: &[Camera],
    preset_mult: f64,
) -> Vec<bool> {
    if coords.len() < 2 {
        return vec![];
    }
    coords
        .windows(2)
        .map(|w| segment_in_camera_zone(w[0], w[1], cameras, preset_mult))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(cam_type: &str, direction: Option<f64>, fov: f64, range_m: f64) -> Camera {
        Camera {
            id: 1, osm_id: None, lat: 45.5, lng: -73.5,
            direction, fov, range_m, cam_type: cam_type.to_string(),
            name: None, operator: None, note: None,
            source: "user".into(), verified: false,
        }
    }

    #[test]
    fn ptz_covers_all_directions_within_range() {
        let c = cam("ptz", None, 360.0, 30.0);
        // Point à ~10m au sud, dans le rayon
        assert!(point_in_camera_zone(45.4999, -73.5, &c, 1.0));
    }

    #[test]
    fn ptz_rejects_beyond_range() {
        let c = cam("ptz", None, 360.0, 10.0);
        // ~100m plus loin que le rayon
        assert!(!point_in_camera_zone(45.501, -73.5, &c, 1.0));
    }

    #[test]
    fn fixed_camera_rejects_point_behind_it() {
        let c = cam("fixed", Some(0.0), 80.0, 30.0); // regarde plein nord
        // Point plein sud du champ de vision → hors du cône
        assert!(!point_in_camera_zone(45.499, -73.5, &c, 1.0));
    }

    #[test]
    fn fixed_camera_accepts_point_in_cone() {
        let c = cam("fixed", Some(0.0), 80.0, 30.0); // regarde plein nord
        assert!(point_in_camera_zone(45.5002, -73.5, &c, 1.0));
    }

    #[test]
    fn preset_multiplier_extends_range() {
        let c = cam("ptz", None, 360.0, 10.0);
        // ~14m au nord : hors portée ×1 (10m) mais dans portée ×2.2 (22m)
        let far = (45.500126, -73.5);
        assert!((haversine_m(45.5, -73.5, far.0, far.1) - 14.0).abs() < 1.0);
        assert!(!point_in_camera_zone(far.0, far.1, &c, 1.0));
        assert!(point_in_camera_zone(far.0, far.1, &c, 2.2));
    }
}

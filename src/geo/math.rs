//! Primitives géométriques pures (Haversine, cap, point de destination).
//! Aucune dépendance aux modèles métier — testable en isolation.

pub(crate) const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// Distance Haversine entre deux points (mètres).
pub fn haversine_m(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlng = (lng2 - lng1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlng / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Azimut (degrés, 0–360) du point (lat1, lng1) vers (lat2, lng2).
pub(crate) fn bearing_deg(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let dlng = (lng2 - lng1).to_radians();
    let y = dlng.sin() * lat2.to_radians().cos();
    let x = lat1.to_radians().cos() * lat2.to_radians().sin()
        - lat1.to_radians().sin() * lat2.to_radians().cos() * dlng.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// Point de destination à partir d'un point, d'un cap et d'une distance.
/// Retourne (lat, lng) en degrés.
pub(crate) fn dest_point(lat: f64, lng: f64, bearing_deg: f64, dist_m: f64) -> (f64, f64) {
    let d = dist_m / EARTH_RADIUS_M;
    let b = bearing_deg.to_radians();
    let lat1 = lat.to_radians();
    let lng1 = lng.to_radians();

    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * b.cos()).asin();
    let lng2 = lng1
        + (b.sin() * d.sin() * lat1.cos())
            .atan2(d.cos() - lat1.sin() * lat2.sin());

    (lat2.to_degrees(), lng2.to_degrees())
}

/// Distance approximative (en degrés euclidiens) d'un point P au segment [A, B].
/// Utilise une correction longitudinale pour la latitude de Montréal (~47°).
/// Suffisant pour trier les caméras par proximité — pas besoin de Haversine exact.
pub fn dist_to_segment_approx(
    p_lat: f64, p_lng: f64,
    a_lat: f64, a_lng: f64,
    b_lat: f64, b_lng: f64,
) -> f64 {
    // cos(47°) ≈ 0.682 : correction distorsion longitude à la latitude de Montréal
    const COS_LAT: f64 = 0.682;
    let dx = (b_lng - a_lng) * COS_LAT;
    let dy = b_lat - a_lat;
    let len2 = dx * dx + dy * dy;
    let (near_lat, near_lng) = if len2 < 1e-12 {
        (a_lat, a_lng)
    } else {
        let t = (((p_lng - a_lng) * COS_LAT) * dx + (p_lat - a_lat) * dy) / len2;
        let t = t.clamp(0.0, 1.0);
        (a_lat + t * (b_lat - a_lat), a_lng + t * (b_lng - a_lng))
    };
    let dlat = p_lat - near_lat;
    let dlng = (p_lng - near_lng) * COS_LAT;
    (dlat * dlat + dlng * dlng).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_zero_for_identical_points() {
        assert_eq!(haversine_m(45.5, -73.5, 45.5, -73.5), 0.0);
    }

    #[test]
    fn haversine_known_distance() {
        // Deux points à ~0.001° de latitude ≈ 111 m
        let d = haversine_m(45.5, -73.5, 45.501, -73.5);
        assert!((d - 111.2).abs() < 1.0, "distance inattendue: {d}");
    }

    #[test]
    fn bearing_north_is_zero() {
        let b = bearing_deg(45.5, -73.5, 45.6, -73.5); // point plus au nord
        assert!(b.abs() < 0.5, "cap attendu ≈0°, obtenu {b}");
    }

    #[test]
    fn bearing_east_is_90() {
        let b = bearing_deg(45.5, -73.5, 45.5, -73.4); // point plus à l'est
        assert!((b - 90.0).abs() < 1.0, "cap attendu ≈90°, obtenu {b}");
    }

    #[test]
    fn dest_point_roundtrip_distance() {
        let (lat2, lng2) = dest_point(45.5, -73.5, 90.0, 100.0);
        let d = haversine_m(45.5, -73.5, lat2, lng2);
        assert!((d - 100.0).abs() < 0.5, "distance parcourue inattendue: {d}");
    }

    #[test]
    fn dist_to_segment_zero_on_segment() {
        // Point au milieu du segment → distance ≈ 0
        let d = dist_to_segment_approx(45.5, -73.5, 45.5, -73.51, 45.5, -73.49);
        assert!(d < 1e-9, "distance attendue ≈0, obtenue {d}");
    }
}

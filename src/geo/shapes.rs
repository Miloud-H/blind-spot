//! Génération de formes simples (cône, cercle) en coordonnées GeoJSON.
//! Fallback utilisé quand aucune donnée bâtiment n'est disponible pour le viewshed.

use super::math::dest_point;

/// Génère un polygone cône en coordonnées GeoJSON [[lng, lat], ...] (fermé).
pub fn build_cone(
    lat: f64,
    lng: f64,
    direction: f64,
    fov: f64,
    range_m: f64,
    steps: usize,
) -> Vec<[f64; 2]> {
    let half = fov / 2.0;
    let mut pts = vec![[lng, lat]];
    for i in 0..=steps {
        let angle = direction - half + (fov * i as f64 / steps as f64);
        let (la, lo) = dest_point(lat, lng, angle, range_m);
        pts.push([lo, la]);
    }
    pts.push([lng, lat]); // fermer le ring
    pts
}

/// Génère un cercle en coordonnées GeoJSON [[lng, lat], ...] (fermé).
pub fn build_circle(lat: f64, lng: f64, range_m: f64, steps: usize) -> Vec<[f64; 2]> {
    let mut pts: Vec<[f64; 2]> = (0..steps)
        .map(|i| {
            let (la, lo) = dest_point(lat, lng, 360.0 * i as f64 / steps as f64, range_m);
            [lo, la]
        })
        .collect();
    if !pts.is_empty() {
        pts.push(pts[0]); // fermer
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cone_closes() {
        let pts = build_cone(45.52, -73.59, 90.0, 70.0, 30.0, 10);
        // Premier et dernier point identiques
        assert_eq!(pts.first(), pts.last());
        // 1 (origine) + 11 (arc) + 1 (close) = 13 points
        assert_eq!(pts.len(), 13);
    }

    #[test]
    fn circle_closes() {
        let pts = build_circle(45.52, -73.59, 25.0, 16);
        assert_eq!(pts.first(), pts.last());
        assert_eq!(pts.len(), 17);
    }
}

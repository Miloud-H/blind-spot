//! Viewshed (line of sight) par ray-casting contre des polygones de bâtiments,
//! et génération des rings GeoJSON envoyés à ORS comme `avoid_polygons`.

use crate::models::{BuildingGeom, Camera};
use super::shapes::{build_circle, build_cone};

/// Retourne t ∈ (0, 1] si le rayon A→B intersecte le segment C→D, sinon None.
/// Coordonnées : x = lng, y = lat (cohérent avec le JS).
fn ray_seg_t(ax: f64, ay: f64, bx: f64, by: f64,
             cx: f64, cy: f64, dx: f64, dy: f64) -> Option<f64> {
    let abx = bx - ax; let aby = by - ay;
    let cdx = dx - cx; let cdy = dy - cy;
    let den = abx * cdy - aby * cdx;
    if den.abs() < 1e-15 { return None; }
    let acx = cx - ax; let acy = cy - ay;
    let t = (acx * cdy - acy * cdx) / den;
    let u = (acx * aby - acy * abx) / den;
    if t > 1e-9 && t <= 1.0 + 1e-9 && u >= -1e-9 && u <= 1.0 + 1e-9 {
        Some(t.min(1.0))
    } else {
        None
    }
}

/// Point-in-polygon pour les polygones de bâtiments stockés en format `[[lat, lng], …]`.
/// Coordonnées internes : x = pts[i][1] (lng), y = pts[i][0] (lat).
fn point_in_bld_pts(lng: f64, lat: f64, pts: &[[f64; 2]]) -> bool {
    let n = pts.len();
    if n < 3 { return false; }
    let mut inside = false;
    let mut j = n.saturating_sub(1);
    for i in 0..n {
        let (xi, yi) = (pts[i][1], pts[i][0]); // lng, lat
        let (xj, yj) = (pts[j][1], pts[j][0]);
        if ((yi > lat) != (yj > lat))
            && (lng < (xj - xi) * (lat - yi) / (yj - yi) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Calcule le viewshed d'une caméra par ray-casting contre des polygones de bâtiments.
///
/// Retourne un ring GeoJSON `[[lng, lat], …]` fermé.
/// Miroir exact de `computeViewshed` côté JavaScript.
///
/// - `direction = None`  → PTZ (360°)
/// - `num_rays`          → résolution angulaire (120 PTZ, ≥60 fixe)
pub fn compute_viewshed(
    lat: f64, lng: f64,
    range_m: f64,
    direction: Option<f64>,
    fov: f64,
    num_rays: usize,
    buildings: &[BuildingGeom],
) -> Vec<[f64; 2]> {
    let is_ptz = direction.is_none();
    let a0 = direction.map(|d| d - fov / 2.0).unwrap_or(0.0);
    let a1 = direction.map(|d| d + fov / 2.0).unwrap_or(360.0);
    let step = (a1 - a0) / num_rays as f64;

    let cos_lat = lat.to_radians().cos();
    let m_lat: f64 = 111_320.0;
    let m_lng = m_lat * cos_lat;
    let cx = lng; let cy = lat;

    // Bâtiments proches — filtre bbox rapide, puis exclusion du bâtiment hôte
    // (caméra posée sur un mur : on exclut le bâtiment qui contient la caméra
    //  sinon les rayons se bloquent immédiatement sur le mur porteur).
    let range_deg = range_m / 111_320.0 + 0.0005;
    let near: Vec<&BuildingGeom> = buildings.iter().filter(|b| {
        b.max_lat >= lat - range_deg && b.min_lat <= lat + range_deg &&
        b.max_lng >= lng - range_deg && b.min_lng <= lng + range_deg &&
        !point_in_bld_pts(lng, lat, &b.pts) // exclure le bâtiment porteur
    }).collect();

    let mut pts = vec![[lng, lat]]; // GeoJSON [lng, lat]

    for i in 0..=num_rays {
        let angle_rad = (a0 + i as f64 * step).to_radians();
        let dx = angle_rad.sin() * range_m / m_lng;
        let dy = angle_rad.cos() * range_m / m_lat;
        let ex = cx + dx; let ey = cy + dy;
        let mut min_t = 1.0f64;

        for b in &near {
            // b.pts = [[lat, lng], …] → x = pts[j][1], y = pts[j][0]
            for j in 0..b.pts.len().saturating_sub(1) {
                let (x3, y3) = (b.pts[j][1],   b.pts[j][0]);
                let (x4, y4) = (b.pts[j+1][1], b.pts[j+1][0]);
                if let Some(t) = ray_seg_t(cx, cy, ex, ey, x3, y3, x4, y4) {
                    if t < min_t { min_t = t; }
                }
            }
        }

        pts.push([cx + dx * min_t, cy + dy * min_t]);
    }

    if !is_ptz { pts.push([lng, lat]); } // fermer le cône
    pts
}

/// Convertit une liste de caméras en rings GeoJSON pour `avoid_polygons` ORS.
/// Chaque ring = Vec<[lng, lat]> fermé.
/// `preset_mult` : multiplicateur de portée (0.5 = conservateur, 1.0 = standard, 2.2 = élevé).
/// `buildings` : slice de bâtiments pour le viewshed LOS.
/// Si vide → fallback sur les formes simples (cercles / cônes).
pub fn cameras_to_ors_rings(
    cameras: &[Camera],
    preset_mult: f64,
    buildings: &[BuildingGeom],
) -> Vec<Vec<[f64; 2]>> {
    cameras.iter().map(|cam| {
        let range = cam.range_m * preset_mult;
        let is_circular = matches!(cam.cam_type.as_str(), "ptz" | "dome" | "panoramic");

        if buildings.is_empty() {
            // Pas de données bâtiments — formes simples (backward compatible)
            return if is_circular {
                build_circle(cam.lat, cam.lng, range, 20)
            } else if let Some(dir) = cam.direction {
                build_cone(cam.lat, cam.lng, dir, cam.fov, range, 10)
            } else {
                build_circle(cam.lat, cam.lng, range * 0.5, 16)
            };
        }

        // Viewshed LOS — polygone arrêté par les bâtiments
        let direction = if is_circular { None } else { cam.direction };
        let fov       = if is_circular { 360.0 } else { cam.fov };
        let num_rays  = if is_circular { 120 } else { (fov as usize).max(60) };
        compute_viewshed(cam.lat, cam.lng, range, direction, fov, num_rays, buildings)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewshed_ptz_without_buildings_is_open_circle() {
        let pts = compute_viewshed(45.52, -73.59, 30.0, None, 360.0, 8, &[]);
        // PTZ sans obstacle : pas de fermeture sur le centre, juste l'arc
        assert_eq!(pts.len(), 1 + 9); // origine + 9 rayons (0..=8)
    }

    #[test]
    fn viewshed_directional_closes_the_cone() {
        let pts = compute_viewshed(45.52, -73.59, 30.0, Some(90.0), 60.0, 8, &[]);
        assert_eq!(pts.first(), pts.last());
    }
}

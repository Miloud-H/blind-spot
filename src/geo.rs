/// Calcul géométrique des cônes de surveillance (Haversine)
/// Reproduit la logique JS du prototype côté serveur.
use crate::models::Camera;

const EARTH_RADIUS_M: f64 = 6_371_000.0;

// ── Primitives géométriques ───────────────────────────────────────────────────

/// Distance Haversine entre deux points (mètres).
pub fn haversine_m(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlng = (lng2 - lng1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlng / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Azimut (degrés, 0–360) du point (lat1, lng1) vers (lat2, lng2).
fn bearing_deg(lat1: f64, lng1: f64, lat2: f64, lng2: f64) -> f64 {
    let dlng = (lng2 - lng1).to_radians();
    let y = dlng.sin() * lat2.to_radians().cos();
    let x = lat1.to_radians().cos() * lat2.to_radians().sin()
        - lat1.to_radians().sin() * lat2.to_radians().cos() * dlng.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

// ── Exposition par segment ────────────────────────────────────────────────────

/// Teste si un point (lat, lng) tombe dans la zone de surveillance d'une caméra.
/// Miroir exact de `isPointInCameraZone` côté JavaScript (cone + cercle PTZ).
fn point_in_single_camera_zone(lat: f64, lng: f64, cam: &Camera, preset_mult: f64) -> bool {
    let range = cam.range_m * preset_mult;
    let dist  = haversine_m(lat, lng, cam.lat, cam.lng);
    if dist > range * 1.15 { return false; } // rejet rapide

    let is_ptz = matches!(cam.cam_type.as_str(), "ptz" | "dome");
    if is_ptz || cam.direction.is_none() {
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
            .any(|&(lat, lng)| point_in_single_camera_zone(lat, lng, cam, preset_mult))
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

/// Point de destination à partir d'un point, d'un cap et d'une distance.
/// Retourne (lat, lng) en degrés.
fn dest_point(lat: f64, lng: f64, bearing_deg: f64, dist_m: f64) -> (f64, f64) {
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

/// Convertit une liste de caméras en rings GeoJSON pour `avoid_polygons` ORS.
/// Chaque ring = Vec<[lng, lat]> fermé.
/// `preset_mult` : multiplicateur de portée (0.5 = conservateur, 1.0 = standard, 2.2 = élevé).
pub fn cameras_to_ors_rings(cameras: &[Camera], preset_mult: f64) -> Vec<Vec<[f64; 2]>> {
    cameras
        .iter()
        .map(|cam| {
            let range = cam.range_m * preset_mult;
            let is_ptz = matches!(cam.cam_type.as_str(), "ptz" | "dome");
            if is_ptz {
                build_circle(cam.lat, cam.lng, range, 20)
            } else if let Some(dir) = cam.direction {
                build_cone(cam.lat, cam.lng, dir, cam.fov, range, 10)
            } else {
                // Caméra fixe sans direction connue → cercle réduit
                build_circle(cam.lat, cam.lng, range * 0.5, 16)
            }
        })
        .collect()
}

// ── Fusion de polygones ───────────────────────────────────────────────────────

/// Structure Union-Find avec path-halving pour grouper les rings qui se chevauchent.
struct UnionFind {
    parent: Vec<usize>,
    rank:   Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self { parent: (0..n).collect(), rank: vec![0; n] }
    }

    /// Recherche avec path-halving (itératif, sans récursion).
    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]]; // path halving
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, x: usize, y: usize) {
        let (px, py) = (self.find(x), self.find(y));
        if px == py { return; }
        match self.rank[px].cmp(&self.rank[py]) {
            std::cmp::Ordering::Less    => self.parent[px] = py,
            std::cmp::Ordering::Greater => self.parent[py] = px,
            std::cmp::Ordering::Equal   => { self.parent[py] = px; self.rank[px] += 1; }
        }
    }
}

/// Convex hull (Andrew's monotone chain) sur un ensemble de points [x, y].
/// Retourne un ring fermé (premier == dernier point).
fn convex_hull(mut pts: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    pts.sort_unstable_by(|a, b| {
        a[0].partial_cmp(&b[0])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a[1].partial_cmp(&b[1]).unwrap_or(std::cmp::Ordering::Equal))
    });
    // Dédupliquer les points identiques (évite les dégénérescences)
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-10 && (a[1] - b[1]).abs() < 1e-10);

    let n = pts.len();
    if n < 3 {
        if !pts.is_empty() { pts.push(pts[0]); }
        return pts;
    }

    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| -> f64 {
        (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    };

    let mut hull: Vec<[f64; 2]> = Vec::with_capacity(n + 1);

    // Coque inférieure
    for &p in &pts {
        while hull.len() >= 2 && cross(hull[hull.len()-2], hull[hull.len()-1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }

    // Coque supérieure
    let lower_len = hull.len() + 1;
    for &p in pts.iter().rev() {
        while hull.len() >= lower_len && cross(hull[hull.len()-2], hull[hull.len()-1], p) <= 0.0 {
            hull.pop();
        }
        hull.push(p);
    }
    hull.pop(); // enlève le duplicata du premier point

    hull.push(hull[0]); // fermer le ring
    hull
}

/// Fusionne les rings qui se chevauchent (détection par bounding box) via convex hull.
///
/// Algorithme :
/// 1. Calcule le bbox de chaque ring
/// 2. Union-Find : relie les rings dont les bboxes se chevauchent
/// 3. Pour chaque groupe, remplace tous les rings par la convex hull de leurs sommets
///
/// Conservatif : la convex hull peut légèrement sur-couvrir par rapport à l'union exacte,
/// ce qui est sûr pour l'évitement (on évite un peu plus, jamais moins).
pub fn merge_overlapping_rings(rings: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    let n = rings.len();
    if n <= 1 { return rings; }

    // Bboxes [min_x, min_y, max_x, max_y]
    let bboxes: Vec<[f64; 4]> = rings.iter().map(|r| {
        r.iter().fold(
            [f64::MAX, f64::MAX, f64::MIN, f64::MIN],
            |[x0, y0, x1, y1], [x, y]| [x0.min(*x), y0.min(*y), x1.max(*x), y1.max(*y)],
        )
    }).collect();

    // Union-Find : relier les rings dont les bboxes se chevauchent
    let mut uf = UnionFind::new(n);
    for i in 0..n {
        let [ax0, ay0, ax1, ay1] = bboxes[i];
        for j in (i + 1)..n {
            let [bx0, by0, bx1, by1] = bboxes[j];
            if ax0 <= bx1 && ax1 >= bx0 && ay0 <= by1 && ay1 >= by0 {
                uf.union(i, j);
            }
        }
    }

    // Grouper par composante connexe
    let mut groups: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        groups.entry(uf.find(i)).or_default().push(i);
    }

    // Fusionner chaque groupe
    groups.into_values()
        .map(|indices| {
            if indices.len() == 1 {
                rings[indices[0]].clone()
            } else {
                let all_pts: Vec<[f64; 2]> = indices.iter()
                    .flat_map(|&i| rings[i].iter().copied())
                    .collect();
                convex_hull(all_pts)
            }
        })
        .collect()
}

// ── Marge de sécurité ORS ────────────────────────────────────────────────────

/// Agrandit un ring GeoJSON autour de son centroïde par un facteur `factor`.
///
/// Utilisé pour créer une marge entre la zone réelle de la caméra et le polygon
/// envoyé à ORS : ORS route clairement à l'extérieur, ce qui évite qu'il passe
/// au bord du polygon (qui est légèrement aplati à cause de la discrétisation).
///
/// L'affichage visuel et le score d'exposition utilisent toujours les rings originaux.
pub fn add_ors_safety_margin(rings: Vec<Vec<[f64; 2]>>, factor: f64) -> Vec<Vec<[f64; 2]>> {
    rings.into_iter().map(|ring| {
        if ring.len() < 2 { return ring; }
        let n = ring.len() as f64;
        let cx = ring.iter().map(|p| p[0]).sum::<f64>() / n;
        let cy = ring.iter().map(|p| p[1]).sum::<f64>() / n;
        ring.iter().map(|&[x, y]| {
            [cx + (x - cx) * factor, cy + (y - cy) * factor]
        }).collect()
    }).collect()
}

// ── Point-in-polygon ─────────────────────────────────────────────────────────

/// Teste si le point (px, py) est à l'intérieur d'un ring GeoJSON fermé [[x,y]…].
/// Algorithme ray-casting (Jordan) — exact pour les polygones simples.
pub fn point_in_polygon(px: f64, py: f64, ring: &[[f64; 2]]) -> bool {
    let n = ring.len();
    if n < 3 { return false; }
    let mut inside = false;
    let mut j = n.saturating_sub(1);
    for i in 0..n {
        let (xi, yi) = (ring[i][0], ring[i][1]);
        let (xj, yj) = (ring[j][0], ring[j][1]);
        if ((yi > py) != (yj > py))
            && (px < (xj - xi) * (py - yi) / (yj - yi) + xi)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Retire les rings dont le polygone contient le point de départ ou d'arrivée.
///
/// Raison : ORS renvoie l'erreur 2010 "Route could not be found" si le point
/// de départ/arrivée est à l'intérieur d'une `avoid_polygon`.
/// On retire uniquement ces rings-là ; les autres restent intacts.
///
/// Retourne `(rings_filtrés, nombre_retiré)`.
pub fn filter_rings_containing_endpoints(
    rings: Vec<Vec<[f64; 2]>>,
    start: (f64, f64), // (lng, lat)
    end: (f64, f64),
) -> (Vec<Vec<[f64; 2]>>, usize) {
    let before = rings.len();
    let filtered: Vec<_> = rings
        .into_iter()
        .filter(|ring| {
            !point_in_polygon(start.0, start.1, ring)
                && !point_in_polygon(end.0, end.1, ring)
        })
        .collect();
    let removed = before - filtered.len();
    (filtered, removed)
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

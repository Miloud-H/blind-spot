//! Opérations sur les rings GeoJSON : fusion des zones qui se chevauchent,
//! marge de sécurité ORS, test point-in-polygon, filtrage des rings englobant
//! les points de départ/arrivée d'une route.

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
    // Comparaison par paire i<j sur indices — needless_range_loop ne s'applique pas ici
    // (on a besoin des deux indices pour indexer `bboxes` et appeler `uf.union`).
    #[allow(clippy::needless_range_loop)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn square(cx: f64, cy: f64, half: f64) -> Vec<[f64; 2]> {
        vec![
            [cx - half, cy - half], [cx + half, cy - half],
            [cx + half, cy + half], [cx - half, cy + half],
            [cx - half, cy - half],
        ]
    }

    #[test]
    fn point_in_polygon_center_is_inside() {
        assert!(point_in_polygon(0.0, 0.0, &square(0.0, 0.0, 1.0)));
    }

    #[test]
    fn point_in_polygon_far_outside() {
        assert!(!point_in_polygon(10.0, 10.0, &square(0.0, 0.0, 1.0)));
    }

    #[test]
    fn merge_keeps_disjoint_rings_separate() {
        let far_apart = vec![square(0.0, 0.0, 0.1), square(100.0, 100.0, 0.1)];
        let merged = merge_overlapping_rings(far_apart);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_combines_overlapping_rings() {
        let overlapping = vec![square(0.0, 0.0, 1.0), square(0.5, 0.5, 1.0)];
        let merged = merge_overlapping_rings(overlapping);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn filter_removes_ring_containing_start() {
        let rings = vec![square(0.0, 0.0, 1.0)];
        let (kept, removed) = filter_rings_containing_endpoints(rings, (0.0, 0.0), (50.0, 50.0));
        assert_eq!(removed, 1);
        assert!(kept.is_empty());
    }

    #[test]
    fn safety_margin_doubles_extent_for_factor_two() {
        let ring = square(0.0, 0.0, 1.0);
        let expanded = &add_ors_safety_margin(vec![ring.clone()], 2.0)[0];

        // L'étendue (max - min) scale linéairement par le facteur, quel que soit
        // le centroïde utilisé comme pivot — invariant robuste à vérifier ici.
        let extent = |pts: &[[f64; 2]]| {
            let xs: Vec<f64> = pts.iter().map(|p| p[0]).collect();
            xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!((extent(expanded) - extent(&ring) * 2.0).abs() < 1e-9);
    }
}

/// Routeur A* piéton sur le graphe OSM pré-pondéré.
/// Coût d'arête = distance × (1 + PENALTY × exposure)
/// où exposure ∈ [0,1] est le score précalculé pour le preset sélectionné.
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Ordering;
use crate::{geo, models::{GraphEdge, RouteResult}};

const CAMERA_PENALTY: f64 = 15.0; // ×16 sur une arête 100% surveillée
const WALK_SPEED_MS:  f64 = 1.4;  // m/s (~5 km/h)

// ── Types internes ────────────────────────────────────────────────────────────

#[derive(Clone)]
struct State {
    f: f64,  // g + h
    g: f64,  // coût réel depuis start
    node: i64,
}

impl PartialEq  for State { fn eq(&self, o: &Self) -> bool { self.node == o.node && self.f == o.f } }
impl Eq         for State {}
impl Ord        for State {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap : on inverse pour BinaryHeap (max-heap par défaut)
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal)
            .then_with(|| self.node.cmp(&o.node))
    }
}
impl PartialOrd for State { fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) } }

// ── Graphe en mémoire ─────────────────────────────────────────────────────────

pub struct AstarRouter {
    nodes: HashMap<i64, (f64, f64)>,        // id → (lat, lng)
    adj:   HashMap<i64, Vec<(i64, f64)>>,   // id → [(voisin, coût)]
}

impl AstarRouter {
    /// Construit le routeur depuis les arêtes chargées depuis la DB.
    /// `avoid` : si false, ignore les scores d'exposition (route directe).
    pub fn new(edges: &[GraphEdge], avoid: bool) -> Self {
        let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
        let mut adj:   HashMap<i64, Vec<(i64, f64)>> = HashMap::new();

        for e in edges {
            nodes.insert(e.from_node, (e.from_lat, e.from_lng));
            nodes.insert(e.to_node,   (e.to_lat,   e.to_lng));

            let exposure = if avoid { e.exposure } else { 0.0 };
            let cost = e.distance_m * (1.0 + CAMERA_PENALTY * exposure);
            adj.entry(e.from_node).or_default().push((e.to_node, cost));
        }

        Self { nodes, adj }
    }

    /// Calcule la route entre deux coordonnées GPS.
    /// Retourne None seulement si le graphe est vide ou entièrement déconnecté.
    pub fn route(
        &self,
        start_lat: f64, start_lng: f64,
        end_lat:   f64, end_lng:   f64,
    ) -> Option<RouteResult> {
        if self.nodes.is_empty() { return None; }

        let start_node = self.nearest_node(start_lat, start_lng)?;
        let end_node   = self.nearest_node(end_lat,   end_lng)?;

        let path = if start_node == end_node {
            vec![start_node]
        } else {
            self.astar(start_node, end_node)?
        };

        // Coordonnées GeoJSON [lng, lat]
        let mut coords: Vec<[f64; 2]> = Vec::with_capacity(path.len() + 2);
        coords.push([start_lng, start_lat]);
        for &nid in &path {
            let (lat, lng) = self.nodes[&nid];
            coords.push([lng, lat]);
        }
        coords.push([end_lng, end_lat]);

        // Distance réelle le long du chemin (nœuds → nœuds)
        let node_dist: f64 = path.windows(2).map(|w| {
            let (lat1, lng1) = self.nodes[&w[0]];
            let (lat2, lng2) = self.nodes[&w[1]];
            geo::haversine_m(lat1, lng1, lat2, lng2)
        }).sum();

        let total_dist = node_dist
            + geo::haversine_m(start_lat, start_lng,
                               self.nodes[&path[0]].0, self.nodes[&path[0]].1)
            + geo::haversine_m(self.nodes[path.last().unwrap()].0,
                               self.nodes[path.last().unwrap()].1,
                               end_lat, end_lng);

        Some(RouteResult {
            coordinates:  coords,
            distance_m:   total_dist,
            duration_sec: total_dist / WALK_SPEED_MS,
        })
    }

    // ── A* ────────────────────────────────────────────────────────────────────

    fn astar(&self, start: i64, end: i64) -> Option<Vec<i64>> {
        let (end_lat, end_lng) = self.nodes[&end];

        let mut heap: BinaryHeap<State> = BinaryHeap::new();
        let mut g_best: HashMap<i64, f64> = HashMap::new();
        let mut came_from: HashMap<i64, i64> = HashMap::new();

        g_best.insert(start, 0.0);
        let h0 = geo::haversine_m(
            self.nodes[&start].0, self.nodes[&start].1,
            end_lat, end_lng,
        );
        heap.push(State { f: h0, g: 0.0, node: start });

        while let Some(State { g, node, .. }) = heap.pop() {
            if node == end {
                return Some(reconstruct(&came_from, end));
            }
            if g > *g_best.get(&node).unwrap_or(&f64::INFINITY) {
                continue;
            }
            if let Some(neighbors) = self.adj.get(&node) {
                for &(next, cost) in neighbors {
                    let new_g = g + cost;
                    if new_g < *g_best.get(&next).unwrap_or(&f64::INFINITY) {
                        g_best.insert(next, new_g);
                        came_from.insert(next, node);
                        let (nlat, nlng) = self.nodes[&next];
                        let h = geo::haversine_m(nlat, nlng, end_lat, end_lng);
                        heap.push(State { f: new_g + h, g: new_g, node: next });
                    }
                }
            }
        }
        None
    }

    fn nearest_node(&self, lat: f64, lng: f64) -> Option<i64> {
        self.nodes.iter()
            .min_by(|(_, &(la, lo)), (_, &(lb, lb2))| {
                geo::haversine_m(lat, lng, la, lo)
                    .partial_cmp(&geo::haversine_m(lat, lng, lb, lb2))
                    .unwrap_or(Ordering::Equal)
            })
            .map(|(&id, _)| id)
    }
}

fn reconstruct(came_from: &HashMap<i64, i64>, end: i64) -> Vec<i64> {
    let mut path = vec![end];
    let mut cur = end;
    while let Some(&prev) = came_from.get(&cur) {
        path.push(prev);
        cur = prev;
    }
    path.reverse();
    path
}

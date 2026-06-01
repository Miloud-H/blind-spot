-- Graphe routier piéton pour le routing A* maison.
-- Nœuds = intersections OSM. Arêtes = segments entre intersections.
-- Les scores d'exposition (exp_*) sont précalculés pour les 3 presets.

CREATE TABLE IF NOT EXISTS routing_nodes (
    id   INTEGER PRIMARY KEY,   -- osm node id
    lat  REAL    NOT NULL,
    lng  REAL    NOT NULL
);

CREATE TABLE IF NOT EXISTS routing_edges (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    from_node    INTEGER NOT NULL REFERENCES routing_nodes(id),
    to_node      INTEGER NOT NULL REFERENCES routing_nodes(id),
    distance_m   REAL    NOT NULL,
    exp_conserv  REAL    NOT NULL DEFAULT 0,  -- exposition portée ×0.5
    exp_standard REAL    NOT NULL DEFAULT 0,  -- exposition portée ×1.0
    exp_high     REAL    NOT NULL DEFAULT 0   -- exposition portée ×2.2
);

CREATE INDEX IF NOT EXISTS idx_routing_nodes_lat ON routing_nodes(lat);
CREATE INDEX IF NOT EXISTS idx_routing_edges_from ON routing_edges(from_node);
CREATE INDEX IF NOT EXISTS idx_routing_edges_to   ON routing_edges(to_node);

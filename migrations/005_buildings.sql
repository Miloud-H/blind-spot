-- Bâtiments OSM pour le calcul de visibilité (Line of Sight / viewshed)
-- geom : JSON [[lat,lng],…] — coordonnées Leaflet (lat en premier)
-- Colonnes bbox pour le filtrage spatial sans extension R*Tree
CREATE TABLE IF NOT EXISTS buildings (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    osm_id  INTEGER UNIQUE NOT NULL,
    min_lat REAL NOT NULL,
    max_lat REAL NOT NULL,
    min_lng REAL NOT NULL,
    max_lng REAL NOT NULL,
    geom    TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_buildings_lat ON buildings (min_lat, max_lat);
CREATE INDEX IF NOT EXISTS idx_buildings_lng ON buildings (min_lng, max_lng);

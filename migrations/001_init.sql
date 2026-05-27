-- BLINDSPOT MTL — schéma SQLite
-- Pas de PostGIS : lat/lng en REAL, bbox filtrée en Rust/SQL pur.

CREATE TABLE IF NOT EXISTS cameras (
    id          INTEGER  PRIMARY KEY AUTOINCREMENT,
    osm_id      INTEGER  UNIQUE,                        -- NULL si contribution communautaire
    lat         REAL     NOT NULL,
    lng         REAL     NOT NULL,
    direction   REAL,                                   -- azimut 0-359°, NULL si PTZ/inconnu
    fov         REAL     NOT NULL DEFAULT 70,           -- FOV horizontal en degrés
    range_m     REAL     NOT NULL DEFAULT 30,           -- portée estimée en mètres
    cam_type    TEXT     NOT NULL DEFAULT 'unknown',    -- 'fixed' | 'ptz' | 'unknown'
    name        TEXT,
    operator    TEXT,
    note        TEXT,
    source      TEXT     NOT NULL DEFAULT 'osm',        -- 'osm' | 'user'
    verified    INTEGER  NOT NULL DEFAULT 0,            -- 0/1 (booléen SQLite)
    created_at  TEXT     NOT NULL DEFAULT (datetime('now'))
);

-- Index composé lat+lng pour les requêtes bbox
CREATE INDEX IF NOT EXISTS idx_cameras_lat_lng ON cameras(lat, lng);
-- Index source pour filtrer user vs osm
CREATE INDEX IF NOT EXISTS idx_cameras_source ON cameras(source);

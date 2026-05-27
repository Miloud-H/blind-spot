-- Table de métadonnées applicatives (clé/valeur générique).
-- Utilisée pour tracker la date du dernier import OSM et déclencher
-- un re-seed automatique après 7 jours.

CREATE TABLE IF NOT EXISTS metadata (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

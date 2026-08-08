-- Cycle de vie des caméras : fraîcheur (last_seen_at) + dédup cross-source ambiguë.

-- Dernière confirmation d'une caméra 'osm'/'inferred' par un reseed Overpass.
-- NULL pour les caméras communautaires (pas de mécanisme de reseed pour celles-ci).
ALTER TABLE cameras ADD COLUMN last_seen_at TEXT;

-- Référence vers une autre caméra jugée être potentiellement le même appareil physique
-- (même zone, même type, mais direction inconnue/incompatible → pas de fusion automatique,
-- juste un signalement pour revue admin).
ALTER TABLE cameras ADD COLUMN possible_duplicate_of INTEGER REFERENCES cameras(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_cameras_last_seen ON cameras(last_seen_at);

-- Backfill : donne aux caméras OSM/inférées existantes un point de départ propre pour
-- le compte à rebours de fraîcheur, plutôt que de les considérer obsolètes dès la migration.
UPDATE cameras SET last_seen_at = datetime('now') WHERE source IN ('osm', 'inferred');

-- Hash SHA-256 de l'IP d'origine (jamais l'IP brute) — déduplication et anti-spam
ALTER TABLE cameras ADD COLUMN created_from TEXT;

-- Signalements dédupliqués : un même hash ne peut signaler la même caméra qu'une fois
CREATE TABLE IF NOT EXISTS camera_reports (
    camera_id  INTEGER NOT NULL REFERENCES cameras(id) ON DELETE CASCADE,
    ip_hash    TEXT    NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(camera_id, ip_hash)
);

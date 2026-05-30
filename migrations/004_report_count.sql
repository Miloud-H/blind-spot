-- Compteur de signalements par caméra
ALTER TABLE cameras ADD COLUMN report_count INTEGER NOT NULL DEFAULT 0;

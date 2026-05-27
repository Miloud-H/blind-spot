-- Corrige les portées des caméras OSM qui ont la valeur par défaut hardcodée (30m).
-- Les nouvelles règles : fixed=38m, ptz=28m (alignées avec BASE_RANGE du frontend).
-- Seules les caméras avec range_m=30 sont mises à jour (celles sans tag camera:range dans OSM).
-- Les caméras avec une portée OSM explicite différente de 30m ne sont pas touchées.

UPDATE cameras
SET range_m = CASE
    WHEN cam_type = 'ptz' THEN 28.0
    ELSE 38.0
END
WHERE source = 'osm'
  AND range_m = 30.0;

// ─── GEO — Primitives géométriques ──────────────────────────────────────────

function toRad(d) { return d * Math.PI / 180; }
function toDeg(r) { return r * 180 / Math.PI; }

function destPoint(lat, lng, bearing, distM) {
  const R = 6371000, d = distM / R, b = toRad(bearing);
  const lat1 = toRad(lat), lng1 = toRad(lng);
  const lat2 = Math.asin(Math.sin(lat1)*Math.cos(d) + Math.cos(lat1)*Math.sin(d)*Math.cos(b));
  const lng2 = lng1 + Math.atan2(Math.sin(b)*Math.sin(d)*Math.cos(lat1), Math.cos(d)-Math.sin(lat1)*Math.sin(lat2));
  return [toDeg(lat2), toDeg(lng2)];
}

function buildCone(lat, lng, direction, fov, rangeM, steps = 24) {
  const half = fov / 2;
  const pts = [[lat, lng]];
  for (let i = 0; i <= steps; i++) pts.push(destPoint(lat, lng, direction - half + (fov * i / steps), rangeM));
  pts.push([lat, lng]);
  return pts;
}

function buildCircle(lat, lng, rangeM, steps = 48) {
  const pts = [];
  for (let i = 0; i <= steps; i++) pts.push(destPoint(lat, lng, 360 * i / steps, rangeM));
  return pts;
}

function haversineDistance(lat1, lng1, lat2, lng2) {
  const R = 6371000;
  const dLat = toRad(lat2 - lat1), dLng = toRad(lng2 - lng1);
  const a = Math.sin(dLat/2)**2 + Math.cos(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.sin(dLng/2)**2;
  return R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
}

function bearingTo(lat1, lng1, lat2, lng2) {
  const dLng = toRad(lng2 - lng1);
  const y = Math.sin(dLng) * Math.cos(toRad(lat2));
  const x = Math.cos(toRad(lat1)) * Math.sin(toRad(lat2))
           - Math.sin(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.cos(dLng);
  return (toDeg(Math.atan2(y, x)) + 360) % 360;
}

// Point-in-polygon — pts = [[lat,lng],…], x = lng, y = lat
function _ptInBldPts(lng, lat, pts) {
  let inside = false, j = pts.length - 1;
  for (let i = 0; i < pts.length; i++) {
    const xi = pts[i][1], yi = pts[i][0];
    const xj = pts[j][1], yj = pts[j][0];
    if ((yi > lat) !== (yj > lat) && lng < (xj - xi) * (lat - yi) / (yj - yi) + xi)
      inside = !inside;
    j = i;
  }
  return inside;
}

// t ∈ (0,1] si rayon A→B coupe segment C→D, sinon null — x = lng, y = lat
function _raySegT(ax, ay, bx, by, cx, cy, dx, dy) {
  const abx = bx-ax, aby = by-ay, cdx = dx-cx, cdy = dy-cy;
  const den = abx*cdy - aby*cdx;
  if (Math.abs(den) < 1e-15) return null;
  const acx = cx-ax, acy = cy-ay;
  const t = (acx*cdy - acy*cdx) / den;
  const u = (acx*aby - acy*abx) / den;
  return (t > 1e-9 && t <= 1 && u >= -1e-9 && u <= 1+1e-9) ? t : null;
}

function parseDirection(val) {
  if (!val) return null;
  const cardinals = { N:0, NE:45, E:90, SE:135, S:180, SW:225, W:270, NW:315 };
  const v = String(val).trim().toUpperCase();
  if (cardinals[v] !== undefined) return cardinals[v];
  const n = parseFloat(v);
  return isNaN(n) ? null : n;
}

function parseFOV(val)   { if (!val) return 70;  const n = parseFloat(val); return isNaN(n) ? 70  : Math.min(180, Math.max(10, n)); }
function parseRange(val) { if (!val) return 30;  const n = parseFloat(val); return isNaN(n) ? 30  : Math.min(200, Math.max(5,  n)); }

function debounce(fn, ms) {
  let timer;
  return (...args) => { clearTimeout(timer); timer = setTimeout(() => fn(...args), ms); };
}

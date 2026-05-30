// ─── VIEWSHED — Bâtiments + Line of Sight ───────────────────────────────────

const BGRID = 0.001; // ~110 m par cellule

let buildings          = [];
let buildingGrid       = null;
let buildingLoadedBbox = null;
const buildingOsmIds   = new Set();

function _buildGrid() {
  const g = new Map();
  for (let i = 0; i < buildings.length; i++) {
    const b = buildings[i];
    let r0 = Infinity, r1 = -Infinity, c0 = Infinity, c1 = -Infinity;
    for (const [la, lo] of b.pts) {
      if (la < r0) r0 = la; if (la > r1) r1 = la;
      if (lo < c0) c0 = lo; if (lo > c1) c1 = lo;
    }
    for (let r = Math.floor(r0/BGRID); r <= Math.floor(r1/BGRID); r++)
      for (let c = Math.floor(c0/BGRID); c <= Math.floor(c1/BGRID); c++) {
        const k = `${r},${c}`;
        if (!g.has(k)) g.set(k, []);
        g.get(k).push(i);
      }
  }
  return g;
}

function _nearBuildings(lat, lng, rangeM) {
  if (!buildingGrid) return buildings;
  const rd = rangeM / 111320 + BGRID;
  const r0 = Math.floor((lat - rd) / BGRID), r1 = Math.floor((lat + rd) / BGRID);
  const c0 = Math.floor((lng - rd) / BGRID), c1 = Math.floor((lng + rd) / BGRID);
  const set = new Set();
  for (let r = r0; r <= r1; r++)
    for (let c = c0; c <= c1; c++) {
      const cands = buildingGrid.get(`${r},${c}`);
      if (cands) for (const i of cands) set.add(i);
    }
  return [...set].map(i => buildings[i]);
}

function computeViewshed(lat, lng, rangeM, direction, fov, numRays = 120) {
  const isPTZ = direction === null;
  const a0    = isPTZ ? 0   : direction - fov / 2;
  const a1    = isPTZ ? 360 : direction + fov / 2;
  const step  = (a1 - a0) / numRays;

  const cosLat = Math.cos(toRad(lat));
  const mLat = 111320, mLng = mLat * cosLat;
  const cx = lng, cy = lat;

  // Exclure le bâtiment porteur — sinon les rayons se bloquent immédiatement sur le mur
  const near = _nearBuildings(lat, lng, rangeM * 1.05)
    .filter(b => !_ptInBldPts(lng, lat, b.pts));

  const pts = [[lat, lng]];
  for (let i = 0; i <= numRays; i++) {
    const rad = toRad(a0 + i * step);
    const dx = Math.sin(rad) * rangeM / mLng;
    const dy = Math.cos(rad) * rangeM / mLat;
    const ex = cx + dx, ey = cy + dy;
    let minT = 1.0;
    for (const b of near) {
      const p = b.pts;
      for (let j = 0; j < p.length - 1; j++) {
        const t = _raySegT(cx, cy, ex, ey, p[j][1], p[j][0], p[j+1][1], p[j+1][0]);
        if (t !== null && t < minT) minT = t;
      }
    }
    pts.push([cy + dy * minT, cx + dx * minT]);
  }
  if (!isPTZ) pts.push([lat, lng]);
  return pts;
}

// ─── MAP INIT ───────────────────────────────────────────────
const map = L.map('map', { center: [45.5231, -73.5982], zoom: 15, preferCanvas: true });

L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
  attribution: '© OpenStreetMap contributors', maxZoom: 19
}).addTo(map);

// ─── STATE ──────────────────────────────────────────────────
let cameras = [];
// Map<idx, { zones: L.Layer[], marker: L.Marker }> — cameras actuellement sur la carte
const renderedCameras = new Map();
let userCameraCount = 0;
let addMode = false;

// ─── LAZY LOADING ───────────────────────────────────────────
// On charge les caméras par viewport (pas toute la ville d'un coup).
// loadedBbox  : zone déjà chargée depuis le backend (null = rien encore)
// cameraIdSet : Set<number> pour éviter les doublons lors des chargements successifs
let loadedBbox  = null; // { s, w, n, e }
const cameraIdSet = new Set();

// ─── BÂTIMENTS — Line of Sight ───────────────────────────────
let buildings        = [];          // [{pts:[[lat,lng]…]}]
let buildingGrid     = null;        // Map<"r,c", number[]> index spatial
let buildingLoadedBbox = null;
const buildingOsmIds = new Set();
const BGRID          = 0.001;       // ~110 m par cellule

// ─── PORTÉE / PRESETS ───────────────────────────────────────
// Portée de base (standard) par type de caméra, en mètres
const BASE_RANGE = { fixed: 38, ptz: 28, unknown: 20 };
// Multiplicateur par scénario
const PRESET_MULT = { conservative: 0.5, standard: 1.0, high: 2.2 };
let rangePreset = 'standard';

function getRange(cam) {
  // Caméra ajoutée manuellement → respecter la saisie, mais la scaler
  if (cam.source === 'user') {
    return (cam.range || 30) * PRESET_MULT[rangePreset];
  }
  // Caméra OSM → table par type × multiplicateur
  const base = BASE_RANGE[cam.type] ?? BASE_RANGE.unknown;
  return base * PRESET_MULT[rangePreset];
}

// ─── UTILS ──────────────────────────────────────────────────
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

// ─── VIEWSHED (Line of Sight) ────────────────────────────────

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

// t ∈ (0,1] si rayon A(ax,ay)→B(bx,by) coupe segment C→D, sinon null
// coordonnées x=lng, y=lat
function _raySegT(ax, ay, bx, by, cx, cy, dx, dy) {
  const abx = bx-ax, aby = by-ay, cdx = dx-cx, cdy = dy-cy;
  const den = abx*cdy - aby*cdx;
  if (Math.abs(den) < 1e-15) return null;
  const acx = cx-ax, acy = cy-ay;
  const t = (acx*cdy - acy*cdx) / den;
  const u = (acx*aby - acy*abx) / den;
  return (t > 1e-9 && t <= 1 && u >= -1e-9 && u <= 1+1e-9) ? t : null;
}

function computeViewshed(lat, lng, rangeM, direction, fov, numRays = 120) {
  const isPTZ = direction === null;
  const a0 = isPTZ ? 0 : direction - fov / 2;
  const a1 = isPTZ ? 360 : direction + fov / 2;
  const step = (a1 - a0) / numRays;

  const cosLat = Math.cos(toRad(lat));
  const mLat = 111320, mLng = mLat * cosLat;
  const cx = lng, cy = lat;
  const near = _nearBuildings(lat, lng, rangeM * 1.05);

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
        // pts[j] = [lat, lng] → y=lat, x=lng
        const t = _raySegT(cx, cy, ex, ey, p[j][1], p[j][0], p[j+1][1], p[j+1][0]);
        if (t !== null && t < minT) minT = t;
      }
    }
    pts.push([cy + dy * minT, cx + dx * minT]);
  }
  if (!isPTZ) pts.push([lat, lng]);
  return pts;
}

async function loadBuildingsForBbox(s, w, n, e) {
  // Pas de rechargement si la zone est déjà couverte
  if (buildingLoadedBbox) {
    const tol = 0.004;
    const { s:bs, w:bw, n:bn, e:be } = buildingLoadedBbox;
    if (s >= bs - tol && w >= bw - tol && n <= bn + tol && e <= be + tol) return;
  }
  const pad = 0.003;
  const q = `[out:json][timeout:25];(way[building](${s-pad},${w-pad},${n+pad},${e+pad}););out geom;`;
  try {
    const res = await fetch('https://overpass-api.de/api/interpreter', {
      method: 'POST',
      body: `data=${encodeURIComponent(q)}`,
      signal: AbortSignal.timeout(25000),
    });
    if (!res.ok) return;
    const data = await res.json();
    let added = 0;
    for (const el of data.elements) {
      if (!el.geometry || el.geometry.length < 3 || buildingOsmIds.has(el.id)) continue;
      buildingOsmIds.add(el.id);
      buildings.push({ pts: el.geometry.map(p => [p.lat, p.lon]) });
      added++;
    }
    buildingLoadedBbox = {
      s: Math.min(buildingLoadedBbox?.s ?? s, s),
      w: Math.min(buildingLoadedBbox?.w ?? w, w),
      n: Math.max(buildingLoadedBbox?.n ?? n, n),
      e: Math.max(buildingLoadedBbox?.e ?? e, e),
    };
    if (added > 0) {
      buildingGrid = _buildGrid();
      // Réaffiche toutes les caméras visibles avec viewshed
      for (const idx of [...renderedCameras.keys()]) unmountCamera(idx);
      syncViewport();
    }
  } catch (_) { /* Overpass indisponible — formes simples conservées */ }
}

// Distance Haversine entre deux points (retourne des mètres)
function haversineDistance(lat1, lng1, lat2, lng2) {
  const R = 6371000;
  const dLat = toRad(lat2 - lat1), dLng = toRad(lng2 - lng1);
  const a = Math.sin(dLat/2)**2 + Math.cos(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.sin(dLng/2)**2;
  return R * 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));
}

// Bearing (azimut) du point 1 vers le point 2, en degrés [0-360]
function bearingTo(lat1, lng1, lat2, lng2) {
  const dLng = toRad(lng2 - lng1);
  const y = Math.sin(dLng) * Math.cos(toRad(lat2));
  const x = Math.cos(toRad(lat1)) * Math.sin(toRad(lat2))
           - Math.sin(toRad(lat1)) * Math.cos(toRad(lat2)) * Math.cos(dLng);
  return (toDeg(Math.atan2(y, x)) + 360) % 360;
}

// Teste si [lat,lng] est dans la zone STANDARD d'une caméra
function isPointInCameraZone(lat, lng, cam) {
  const isPTZ = cam.type === 'ptz' || cam.type === 'dome';
  const base  = cam.source === 'user'
    ? (cam.range || 30)
    : (BASE_RANGE[cam.type] ?? BASE_RANGE.unknown);
  const rangeM = base * PRESET_MULT.standard;

  const dist = haversineDistance(lat, lng, cam.lat, cam.lng);
  if (dist > rangeM * 1.15) return false; // rejet rapide

  if (isPTZ || cam.direction === null) return dist <= rangeM;

  // Caméra directionnelle : vérifier le cône
  const bearing = bearingTo(cam.lat, cam.lng, lat, lng);
  let diff = Math.abs(bearing - cam.direction);
  if (diff > 180) diff = 360 - diff;
  return diff <= (cam.fov || 70) / 2 && dist <= rangeM;
}

// Calcule le score d'exposition d'une route (coords = [[lat,lng]...])
// Retourne { pct, exposedM, totalM }
function computeExposureScore(coords) {
  let exposedM = 0, totalM = 0;
  for (let i = 0; i < coords.length - 1; i++) {
    const [la, lo] = coords[i];
    const segM = haversineDistance(la, lo, coords[i+1][0], coords[i+1][1]);
    totalM += segM;
    if (cameras.some(cam => isPointInCameraZone(la, lo, cam))) exposedM += segM;
  }
  return {
    pct: totalM > 0 ? Math.round(exposedM / totalM * 100) : 0,
    exposedM: Math.round(exposedM),
    totalM:   Math.round(totalM)
  };
}

// Affiche le score dans l'UI
function displayExposureScore(score, avoidMode) {
  const sec = document.getElementById('exposure-section');
  if (!avoidMode || score.totalM === 0) { sec.style.display = 'none'; return; }

  const pct = score.pct;
  const color = pct < 10 ? 'var(--green)' : pct < 35 ? 'var(--amber)' : 'var(--red)';
  const verdict = pct < 10 ? '✓ Itinéraire sûr'
                : pct < 35 ? '⚠ Partiellement exposé'
                :             '⚠ Fortement exposé';

  document.getElementById('exposure-fill').style.width   = `${pct}%`;
  document.getElementById('exposure-fill').style.background = color;
  document.getElementById('exposure-pct').textContent   = `${pct}%`;
  document.getElementById('exposure-pct').style.color   = color;
  document.getElementById('exposure-detail').textContent = `${score.exposedM} m exposés / ${score.totalM} m`;
  const v = document.getElementById('exposure-verdict');
  v.textContent = verdict; v.style.color = color;
  sec.style.display = 'block';
}

function parseDirection(val) {
  if (!val) return null;
  const cardinals = { N:0, NE:45, E:90, SE:135, S:180, SW:225, W:270, NW:315 };
  const v = String(val).trim().toUpperCase();
  if (cardinals[v] !== undefined) return cardinals[v];
  const n = parseFloat(v);
  return isNaN(n) ? null : n;
}
function parseFOV(val) { if (!val) return 70; const n = parseFloat(val); return isNaN(n) ? 70 : Math.min(180, Math.max(10, n)); }
function parseRange(val) { if (!val) return 30; const n = parseFloat(val); return isNaN(n) ? 30 : Math.min(200, Math.max(5, n)); }

// ─── RENDER CAMERA ──────────────────────────────────────────
// 3 zones concentriques : extérieure (étendue) → intérieure (réduite)
// Rendu outer-first (high), inner-last (conservative = rouge, dessiné au dessus)
const ZONE_STYLES = {
  fixed: [
    { key: 'high',         fill: 'rgba(50,210,50,0.13)',  stroke: 'rgba(50,210,50,0.55)',  w: 0.8, dash: '4,6' },
    { key: 'standard',     fill: 'rgba(255,165,0,0.26)',  stroke: 'rgba(255,165,0,0.72)',  w: 1.1, dash: null  },
    { key: 'conservative', fill: 'rgba(255,40,40,0.38)',  stroke: 'rgba(255,40,40,0.88)',  w: 1.5, dash: null  },
  ],
  ptz: [
    { key: 'high',         fill: 'rgba(50,210,50,0.11)',  stroke: 'rgba(50,210,50,0.48)',  w: 0.8, dash: '4,6' },
    { key: 'standard',     fill: 'rgba(255,165,0,0.24)',  stroke: 'rgba(255,165,0,0.65)',  w: 1.1, dash: null  },
    { key: 'conservative', fill: 'rgba(255,40,40,0.36)',  stroke: 'rgba(255,40,40,0.82)',  w: 1.4, dash: null  },
  ],
};

function renderCamera(cam) {
  const { id, lat, lng, direction, fov, type, name, source, note } = cam;
  const isPTZ = type === 'ptz' || type === 'dome';
  const hasDir = direction !== null && !isPTZ;
  const isUser = source === 'user';
  const styles = isPTZ ? ZONE_STYLES.ptz : ZONE_STYLES.fixed;

  const savedPreset = rangePreset;
  const zoneLayers = [];

  // Rendu extérieur → intérieur (inner dessiné en dernier = au dessus)
  for (const z of styles) {
    rangePreset = z.key;
    const rangeM = getRange(cam);
    const opts = { fillColor: z.fill, fillOpacity: 1, color: z.stroke, weight: z.w, dashArray: z.dash };

    let poly;
    if (buildings.length > 0) {
      // Viewshed LOS — zones arrêtées par les bâtiments
      const vDir  = hasDir ? direction : null;
      const vFov  = hasDir ? (fov || 70) : 360;
      const nRays = isPTZ ? 180 : (hasDir ? Math.max(60, Math.round(vFov)) : 120);
      poly = L.polygon(computeViewshed(lat, lng, rangeM, vDir, vFov, nRays), opts);
    } else if (isPTZ || !hasDir) {
      poly = L.polygon(buildCircle(lat, lng, rangeM, 36), opts);
    } else {
      poly = L.polygon(buildCone(lat, lng, direction, fov || 70, rangeM, 24), opts);
    }
    // Ne pas addTo(map) ici — géré par mountCamera
    zoneLayers.push(poly);
  }
  rangePreset = savedPreset;

  // Marqueur — couleur selon la source
  const isInferred = source === 'inferred';
  const dotColor = isUser     ? '#ffb300'   // communauté  → amber
                 : isInferred ? '#00b8d4'   // déduite     → cyan
                 : isPTZ      ? '#ff8c00'   // PTZ OSM     → orange
                 :              '#ff3131';  // fixe OSM    → rouge
  const camIcon = L.divIcon({
    html: `<div style="width:8px;height:8px;background:${dotColor};border-radius:50%;border:1px solid rgba(255,255,255,0.35);box-shadow:0 0 6px ${dotColor};"></div>`,
    iconSize: [8,8], iconAnchor: [4,4], className: ''
  });
  const marker = L.marker([lat, lng], { icon: camIcon, zIndexOffset: 100 });

  const baseRangeM = Math.round(BASE_RANGE[type] ?? BASE_RANGE.unknown);
  const dirTxt = hasDir ? `${Math.round(direction)}° (FOV ${fov||70}°)` : (isPTZ ? '360° PTZ' : 'Inconnue');
  const popupHtml = `
    <div class="popup-title">📹 ${name || 'Caméra de surveillance'}</div>
    <div class="popup-row">Type: <span>${isPTZ ? 'PTZ / Dôme' : 'Fixe'}</span></div>
    <div class="popup-row">Direction: <span>${dirTxt}</span></div>
    <div class="popup-row">Portée estimée: <span>${Math.round(baseRangeM*0.5)}–${Math.round(baseRangeM*2.2)} m</span></div>
    <div class="popup-row">Source: <span>${
      source === 'user'     ? '👤 Communauté' :
      source === 'inferred' ? '🔍 Déduite' :
                              '🗺 OSM'
    }</span></div>
    ${note ? `<div class="popup-row">Note: <span>${note}</span></div>` : ''}
    <div class="popup-row" style="margin-top:6px;font-size:10px;color:var(--text-dim)">${lat.toFixed(5)}, ${lng.toFixed(5)}</div>
    <button onclick="reportCamera(${id})" style="margin-top:8px;width:100%;padding:4px 8px;background:transparent;border:1px solid rgba(255,49,49,0.35);color:var(--red);font-size:9px;letter-spacing:1px;cursor:pointer;font-family:inherit;">⚠ SIGNALER COMME RETIRÉE</button>`;
  marker.bindPopup(popupHtml, { maxWidth: 220 });
  zoneLayers.forEach(p => p.on('click', () => marker.openPopup()));
  // Ne pas addTo(map) ici — géré par mountCamera

  return { zones: zoneLayers, marker };
}

// ─── VIEWPORT CULLING ───────────────────────────────────────
// Monte les layers d'une caméra sur la carte et les stocke dans renderedCameras.
function mountCamera(cam, idx) {
  if (renderedCameras.has(idx)) return;
  const { zones, marker } = renderCamera(cam);
  zones.forEach(z => z.addTo(map));
  marker.addTo(map);
  renderedCameras.set(idx, { zones, marker });
}

// Retire les layers de la carte et libère l'entrée.
function unmountCamera(idx) {
  const entry = renderedCameras.get(idx);
  if (!entry) return;
  entry.zones.forEach(l => map.removeLayer(l));
  map.removeLayer(entry.marker);
  renderedCameras.delete(idx);
}

// Synchronise le rendu avec le viewport actuel (+ 35% de marge).
function syncViewport() {
  const bounds = map.getBounds().pad(0.35);

  // Retirer les caméras sorties du viewport
  for (const idx of [...renderedCameras.keys()]) {
    const cam = cameras[idx];
    if (cam && !bounds.contains([cam.lat, cam.lng])) unmountCamera(idx);
  }

  // Afficher les caméras entrées dans le viewport
  cameras.forEach((cam, idx) => {
    if (!renderedCameras.has(idx) && bounds.contains([cam.lat, cam.lng])) {
      mountCamera(cam, idx);
    }
  });
}

// ─── LIST & STATS ───────────────────────────────────────────
function updateList() {
  const list = document.getElementById('camera-list');
  list.innerHTML = '';
  [...cameras].reverse().forEach((cam, i) => {
    const item = document.createElement('div');
    item.className = 'camera-item';
    const isPTZ = cam.type === 'ptz' || cam.type === 'dome';
    const icon = isPTZ ? '🔄' : (cam.direction !== null ? '📹' : '📷');
    item.innerHTML = `
      <span class="camera-icon">${icon}</span>
      <div class="camera-info">
        <div class="camera-name">${cam.name || 'Caméra #' + (cameras.length - i)}</div>
        <div class="camera-meta">${cam.lat.toFixed(4)}, ${cam.lng.toFixed(4)}</div>
      </div>
      <span class="camera-tag ${cam.source}">${cam.source === 'user' ? 'USER' : 'OSM'}</span>`;
    item.addEventListener('click', () => map.setView([cam.lat, cam.lng], 18, { animate: true }));
    list.appendChild(item);
  });
}

function updateStats() {
  document.getElementById('stat-total').textContent = cameras.length;
  document.getElementById('stat-cones').textContent = cameras.filter(c => c.direction !== null && c.type !== 'ptz').length;
  document.getElementById('stat-ptz').textContent = cameras.filter(c => c.type === 'ptz' || c.type === 'dome').length;
  document.getElementById('stat-user').textContent = userCameraCount;
  // Stat caméras déduites (optionnelle — affichée si présentes)
  const inferredCount = cameras.filter(c => c.source === 'inferred').length;
  const el = document.getElementById('stat-inferred');
  if (el) el.textContent = inferredCount;
}

// ─── TOAST ──────────────────────────────────────────────────
function showToast(msg) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.classList.add('show');
  setTimeout(() => t.classList.remove('show'), 2800);
}

// ─── MOBILE BOTTOM SHEET ────────────────────────────────────
const sidebarEl = document.getElementById('sidebar');

function isMobile() { return window.innerWidth <= 768; }

function openMobilePanel()  { if (isMobile()) sidebarEl.classList.add('panel-open'); }
function closeMobilePanel() { if (isMobile()) sidebarEl.classList.remove('panel-open'); }
function toggleMobilePanel() { if (isMobile()) sidebarEl.classList.toggle('panel-open'); }

document.getElementById('sidebar-handle').addEventListener('click', toggleMobilePanel);

// Tap sur les onglets : ouvre le panel s'il est fermé, sinon change d'onglet normalement
document.querySelector('.tabs').addEventListener('click', e => {
  if (isMobile() && !sidebarEl.classList.contains('panel-open')) {
    openMobilePanel();
    e.stopPropagation();
  }
});

// Swipe vertical sur le handle et les onglets
(function setupSwipe() {
  const targets = [document.getElementById('sidebar-handle'), document.querySelector('.tabs')];
  targets.forEach(el => {
    let startY = 0;
    el.addEventListener('touchstart', e => { startY = e.touches[0].clientY; }, { passive: true });
    el.addEventListener('touchend', e => {
      if (!isMobile()) return;
      const delta = startY - e.changedTouches[0].clientY; // positif = swipe vers le haut
      if (Math.abs(delta) < 20) return; // geste trop court → ignorer
      if (delta > 0) openMobilePanel();
      else closeMobilePanel();
    }, { passive: true });
  });
})();

// ─── TABS ───────────────────────────────────────────────────
function switchTab(name) {
  document.getElementById('tab-route').classList.toggle('active', name === 'route');
  document.getElementById('tab-cams').classList.toggle('active', name === 'cams');
  document.getElementById('content-route').style.display = name === 'route' ? 'flex' : 'none';
  document.getElementById('content-cams').style.display = name === 'cams' ? 'flex' : 'none';
  openMobilePanel();
}

// ─── PRESET HANDLERS ────────────────────────────────────────
function rerenderCameras() {
  for (const idx of [...renderedCameras.keys()]) unmountCamera(idx);
  syncViewport();
}

function setPreset(name) {
  rangePreset = name;
  ['conservative', 'standard', 'high'].forEach(p => {
    const active = p === name;
    document.getElementById(`preset-${p}`)?.classList.toggle('active', active);
    document.getElementById(`preset-${p}-p`)?.classList.toggle('active', active);
  });
  // Le rendu affiche toujours les 3 zones simultanément.
  // Ce preset contrôle uniquement l'agressivité de l'évitement au routing.
  const labels = {
    conservative: 'Évitement réduit — zones internes uniquement',
    standard:     'Évitement standard',
    high:         'Évitement maximal — zones étendues'
  };
  showToast(`◎ ${labels[name]}`);
  updateRouteHash();
}

// ─── CHARGEMENT CAMÉRAS (depuis le backend, par viewport) ────
// Le backend est la source de vérité (seed Overpass au démarrage).
// On charge uniquement la zone visible + 60 % de padding, et on enrichit
// au fil des déplacements sans recharger ce qui est déjà en mémoire.

/**
 * Charge les caméras pour un bbox depuis /api/cameras.
 * Ignore les cameras déjà présentes (déduplication par id).
 * @param {boolean} isInitial  — true lors du premier chargement (affiche les indicateurs)
 */
async function loadCamerasForBbox(s, w, n, e, isInitial = false) {
  if (isInitial) {
    document.getElementById('dot-osm').className = 'dot loading';
    document.getElementById('status-osm').textContent = 'CHARGEMENT...';
  }

  try {
    const res = await fetch(`/api/cameras?bbox=${s},${w},${n},${e}`, {
      signal: AbortSignal.timeout(15000),
    });
    if (isInitial) document.getElementById('loading').style.display = 'none';
    if (!res.ok) throw new Error(`HTTP ${res.status}`);

    const data = await res.json();
    let added = 0;

    data.forEach(c => {
      if (cameraIdSet.has(c.id)) return; // déjà en mémoire
      cameraIdSet.add(c.id);
      cameras.push({
        id:        c.id,
        lat:       c.lat,
        lng:       c.lng,
        direction: c.direction,
        fov:       c.fov       ?? 70,
        range:     c.range_m   ?? 30,
        type:      c.cam_type  ?? 'unknown',
        name:      c.name,
        source:    c.source,
        note:      c.note,
      });
      if (c.source === 'user') userCameraCount++;
      added++;
    });

    // Étendre le bbox chargé pour couvrir cette zone
    loadedBbox = {
      s: Math.min(loadedBbox?.s ?? s, s),
      w: Math.min(loadedBbox?.w ?? w, w),
      n: Math.max(loadedBbox?.n ?? n, n),
      e: Math.max(loadedBbox?.e ?? e, e),
    };

    if (added > 0) {
      syncViewport();
      updateStats();
      updateList();
      _tryOpenHighlight();
    }

    // Bâtiments en arrière-plan — déclenche un re-rendu avec LOS quand prêt
    loadBuildingsForBbox(s, w, n, e);

    if (isInitial) {
      const osmCount = cameras.filter(c => c.source === 'osm').length;
      document.getElementById('dot-osm').className = 'dot';
      document.getElementById('status-osm').textContent = `OSM: ${osmCount} CAMÉRAS`;
      if (osmCount === 0) {
        showToast('⚠ Import OSM en cours — rafraîchir dans quelques secondes');
      } else {
        showToast(`✓ ${osmCount} caméras chargées`);
      }
    }
  } catch (e) {
    if (isInitial) {
      document.getElementById('loading').style.display = 'none';
      document.getElementById('dot-osm').className = 'dot error';
      document.getElementById('status-osm').textContent = 'BACKEND HORS-LIGNE';
      console.warn('loadCameras:', e);
      showToast('⚠ Backend inaccessible — données de démo');
      loadDemoData();
    }
    // Hors chargement initial : échec silencieux (on garde les données existantes)
  }
}

/** Chargement initial : viewport actuel + 60 % de padding. */
async function loadCameras() {
  const bounds = map.getBounds().pad(0.6);
  await loadCamerasForBbox(
    bounds.getSouth(), bounds.getWest(),
    bounds.getNorth(), bounds.getEast(),
    /* isInitial */ true,
  );
}

/**
 * Retourne true si le viewport s'étend hors du bbox déjà chargé
 * (avec une tolérance de 0.005° ≈ 500 m pour éviter les micro-chargements).
 */
function viewportNeedsLoad(bounds) {
  if (!loadedBbox) return true;
  const tol = 0.005;
  return (
    bounds.getSouth() < loadedBbox.s - tol ||
    bounds.getWest()  < loadedBbox.w - tol ||
    bounds.getNorth() > loadedBbox.n + tol ||
    bounds.getEast()  > loadedBbox.e + tol
  );
}

function loadDemoData() {
  [
    { lat: 45.5088, lng: -73.5540, direction: 220, fov: 70, range: 35, type: 'fixed', name: 'SPVM - Ste-Catherine', source: 'osm', note: 'Caméra de sécurité urbaine' },
    { lat: 45.5120, lng: -73.5610, direction: null, fov: 70, range: 25, type: 'ptz', name: 'Dôme PTZ - Guy-Concordia', source: 'osm', note: null },
    { lat: 45.5060, lng: -73.5720, direction: 90, fov: 80, range: 40, type: 'fixed', name: 'Commerce - Atwater', source: 'osm', note: null },
    { lat: 45.5230, lng: -73.5860, direction: 315, fov: 60, range: 30, type: 'fixed', name: 'Caméra fixe', source: 'osm', note: null },
    { lat: 45.5195, lng: -73.5780, direction: null, fov: 70, range: 20, type: 'unknown', name: 'Caméra inconnue', source: 'osm', note: null },
    { lat: 45.5160, lng: -73.5650, direction: 180, fov: 90, range: 45, type: 'fixed', name: 'Entrée parking', source: 'osm', note: null },
  ].forEach(c => cameras.push(c));
  syncViewport();
  updateStats(); updateList();
  showToast('Mode démo — données simulées pour Montréal');
}

// ─── ADD CAMERA MODE ────────────────────────────────────────
const btnAdd = document.getElementById('btn-add-mode');
const btnCancel = document.getElementById('btn-cancel');
const modeBadge = document.getElementById('mode-badge');

btnAdd.addEventListener('click', () => {
  addMode = true;
  routePickMode = null; clearPickButtons();
  map.getContainer().style.cursor = 'crosshair';
  modeBadge.textContent = '⊕ CLIQUER POUR PLACER'; modeBadge.classList.add('active');
  btnAdd.style.display = 'none'; btnCancel.style.display = 'block';
  showToast('Cliquer sur la carte pour placer la caméra');
});

btnCancel.addEventListener('click', () => {
  addMode = false;
  stopOrientTracking();
  map.getContainer().style.cursor = '';
  modeBadge.classList.remove('active');
  btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
});

// ─── COMPASS ────────────────────────────────────────────────
const compass  = document.getElementById('compass');
const needle   = document.getElementById('compass-needle');
const dirInput = document.getElementById('cam-direction');

function setDirection(deg) {
  const v = ((Math.round(deg) % 360) + 360) % 360;
  dirInput.value = v;
  needle.style.transform = `translateX(-50%) translateY(-100%) rotate(${v}deg)`;
}

compass.addEventListener('click', (e) => {
  stopOrientTracking();
  const rect = compass.getBoundingClientRect();
  const cx = rect.left + rect.width / 2, cy = rect.top + rect.height / 2;
  setDirection(toDeg(Math.atan2(e.clientX - cx, -(e.clientY - cy))) + 360);
});
dirInput.addEventListener('input', () => {
  stopOrientTracking();
  setDirection(parseFloat(dirInput.value) || 0);
});
document.getElementById('cam-type').addEventListener('change', (e) => {
  document.getElementById('direction-group').style.display =
    (e.target.value === 'ptz' || e.target.value === 'unknown') ? 'none' : 'block';
});

// ─── ORIENTATION TÉLÉPHONE (compas) ─────────────────────────
let orientActive  = false;
let orientGotHit  = false; // premier signal reçu
let orientTimeout = null;

function _getHeading(e) {
  // iOS Safari : webkitCompassHeading — degrés CW depuis le nord géographique
  if (typeof e.webkitCompassHeading === 'number' && e.webkitCompassHeading >= 0) {
    return e.webkitCompassHeading;
  }
  // Android Chrome : deviceorientationabsolute → e.absolute === true
  // alpha = rotation CCW autour de Z depuis le nord → heading = 360 - alpha
  if (e.absolute === true && typeof e.alpha === 'number' && e.alpha !== null) {
    return (360 - e.alpha + 360) % 360;
  }
  return null;
}

function _onOrient(e) {
  if (!orientActive) return;
  const h = _getHeading(e);
  if (h === null) return;
  if (!orientGotHit) {
    orientGotHit = true;
    clearTimeout(orientTimeout);
    showToast('◎ Compas actif — pointez vers la caméra');
  }
  setDirection(h);
}

function _setOrientBtnState(active) {
  const btn = document.getElementById('btn-orient');
  if (!btn) return;
  btn.textContent = active ? '🔒 COMPAS ACTIF — toucher pour verrouiller' : '📡 UTILISER LE COMPAS';
  btn.classList.toggle('active', active);
}

function stopOrientTracking() {
  if (!orientActive) return;
  orientActive = false;
  clearTimeout(orientTimeout);
  window.removeEventListener('deviceorientationabsolute', _onOrient, true);
  window.removeEventListener('deviceorientation',         _onOrient, true);
  _setOrientBtnState(false);
}

async function startOrientTracking() {
  if (!window.DeviceOrientationEvent) {
    showToast('⚠ Boussole non disponible sur cet appareil'); return;
  }
  // iOS 13+ exige une permission déclenchée par un geste utilisateur
  if (typeof DeviceOrientationEvent.requestPermission === 'function') {
    try {
      const p = await DeviceOrientationEvent.requestPermission();
      if (p !== 'granted') { showToast('⚠ Permission boussole refusée'); return; }
    } catch { showToast('⚠ Permission boussole refusée'); return; }
  }
  orientActive = true;
  orientGotHit = false;
  // Un seul handler pour les deux événements — _getHeading filtre selon le type
  window.addEventListener('deviceorientationabsolute', _onOrient, true);
  window.addEventListener('deviceorientation',         _onOrient, true);
  _setOrientBtnState(true);
  // Si aucun signal absolu reçu après 2 s, avertir
  orientTimeout = setTimeout(() => {
    if (orientActive && !orientGotHit) {
      showToast('⚠ Signal boussole absent — utilisez la roue');
      stopOrientTracking();
    }
  }, 2000);
}

document.getElementById('btn-orient').addEventListener('click', () => {
  if (orientActive) stopOrientTracking();
  else startOrientTracking();
});

// ─── PERSISTANCE CAMÉRAS COMMUNAUTAIRES ─────────────────────
// Le backend (SQLite) est la source de vérité.
// Les caméras user sont chargées dans loadCameras() via GET /api/cameras.
// localStorage sert uniquement de fallback si le backend est indisponible.

async function persistCamera(cam) {
  try {
    const res = await fetch('/api/cameras', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        lat: cam.lat, lng: cam.lng, direction: cam.direction,
        fov: cam.fov, range_m: cam.range,
        cam_type: cam.type, name: cam.name, note: cam.note
      }),
      signal: AbortSignal.timeout(5000)
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
  } catch (e) {
    // Fallback localStorage si le backend est indisponible
    console.warn('persistCamera backend failed:', e);
    try {
      const stored = JSON.parse(localStorage.getItem('blindspot_community_cameras') || '[]');
      stored.push({ ...cam });
      localStorage.setItem('blindspot_community_cameras', JSON.stringify(stored));
    } catch (e2) { console.warn('localStorage write:', e2); }
  }
}

// ─── GÉOLOCALISATION (suivi continu) ─────────────────────────
let userLocCircle = null, userLocDot = null;
let geoWatchId = null;   // null = off
let geoFollow  = false;  // true = carte suit la position

function _updateGeoMarkers(lat, lng, accuracy) {
  if (userLocCircle) { map.removeLayer(userLocCircle); map.removeLayer(userLocDot); }
  userLocCircle = L.circle([lat, lng], {
    radius: accuracy, color: '#00b8d4', fillColor: '#00b8d4',
    fillOpacity: 0.1, weight: 1.5, dashArray: '4,4'
  }).addTo(map);
  userLocDot = L.circleMarker([lat, lng], {
    radius: 7, color: '#fff', fillColor: '#00b8d4',
    fillOpacity: 1, weight: 2
  }).addTo(map);
  userLocCircle.bindPopup(
    `<div class="popup-title">📍 Votre position</div>` +
    `<div class="popup-row">Précision: <span>±${Math.round(accuracy)} m</span></div>`
  );
  userLocDot.on('click', () => userLocCircle.openPopup());
}

function _geoSetBtn(state) {
  // state: 'off' | 'locating' | 'follow' | 'nofollow'
  const btn = document.getElementById('geoloc-btn');
  btn.classList.remove('locating', 'geo-follow', 'geo-nofollow');
  if (state === 'locating') btn.classList.add('locating');
  else if (state === 'follow')   btn.classList.add('geo-follow');
  else if (state === 'nofollow') btn.classList.add('geo-nofollow');
}

function _stopGeoWatch() {
  if (geoWatchId !== null) {
    navigator.geolocation.clearWatch(geoWatchId);
    geoWatchId = null;
  }
  geoFollow = false;
  _geoSetBtn('off');
  if (userLocCircle) { map.removeLayer(userLocCircle); userLocCircle = null; }
  if (userLocDot)    { map.removeLayer(userLocDot);    userLocDot    = null; }
}

function locateUser() {
  if (!navigator.geolocation) { showToast('⚠ Géolocalisation non supportée'); return; }

  // Clic bouton : off→follow, follow→off, nofollow→follow (recentrer)
  if (geoWatchId === null) {
    // Démarrer le suivi
    _geoSetBtn('locating');
    showToast('◎ Localisation en cours…');

    geoWatchId = navigator.geolocation.watchPosition(
      ({ coords: { latitude: lat, longitude: lng, accuracy } }) => {
        const first = !userLocDot;
        _updateGeoMarkers(lat, lng, accuracy);
        if (first) {
          _geoSetBtn('follow');
          geoFollow = true;
          map.setView([lat, lng], 17, { animate: true });
          showToast(`✓ Suivi GPS actif (±${Math.round(accuracy)} m)`);
        } else if (geoFollow) {
          map.panTo([lat, lng], { animate: true, duration: 0.5 });
        }
      },
      (err) => {
        _stopGeoWatch();
        const msgs = { 1: 'Permission refusée', 2: 'Position indisponible', 3: 'Délai dépassé' };
        showToast(`⚠ Géoloc : ${msgs[err.code] || err.message}`);
      },
      { enableHighAccuracy: true, maximumAge: 5000, timeout: 15000 }
    );
  } else if (geoFollow) {
    // Arrêter complètement
    _stopGeoWatch();
    showToast('✕ Suivi GPS arrêté');
  } else {
    // Reprendre le centrage
    geoFollow = true;
    _geoSetBtn('follow');
    if (userLocDot) map.setView(userLocDot.getLatLng(), map.getZoom(), { animate: true });
  }
}

// Pause du suivi de carte quand l'utilisateur fait glisser
map.on('dragstart', () => {
  if (geoWatchId !== null && geoFollow) {
    geoFollow = false;
    _geoSetBtn('nofollow');
  }
});

// ─── SIGNALEMENT CAMÉRA ─────────────────────────────────────
async function reportCamera(id) {
  try {
    const res = await fetch(`/api/cameras/${id}/report`, { method: 'POST' });
    if (res.ok) showToast('✓ Signalement enregistré — merci');
    else        showToast('⚠ Erreur lors du signalement');
  } catch { showToast('⚠ Erreur réseau'); }
}

// ─── HISTORIQUE DES TRAJETS ──────────────────────────────────
const HISTORY_KEY = 'blindspot_history';
const HISTORY_MAX = 10;

function loadHistory() {
  try { return JSON.parse(localStorage.getItem(HISTORY_KEY) || '[]'); }
  catch { return []; }
}

function saveToHistory(entry) {
  const list = loadHistory();
  list.unshift({ ...entry, id: Date.now() });
  localStorage.setItem(HISTORY_KEY, JSON.stringify(list.slice(0, HISTORY_MAX)));
  renderHistory();
}

function deleteFromHistory(id) {
  const list = loadHistory().filter(e => e.id !== id);
  localStorage.setItem(HISTORY_KEY, JSON.stringify(list));
  renderHistory();
}

function renderHistory() {
  const list = loadHistory();
  const section = document.getElementById('history-section');
  const container = document.getElementById('history-list');
  if (!list.length) { section.style.display = 'none'; return; }
  section.style.display = 'block';
  container.innerHTML = list.map(e => {
    const ago = timeAgo(e.id);
    const dist = e.distKm < 1 ? `${Math.round(e.distKm * 1000)} m` : `${e.distKm.toFixed(2)} km`;
    const dur  = `~${Math.ceil(e.durSec / 60)} min`;
    const preset = e.preset !== 'standard' ? ` · ${e.preset}` : '';
    return `<div class="history-item" onclick="restoreHistory(${e.id})">
      <div class="history-badges">
        <div class="history-badge a">A</div>
        <div class="history-badge b">B</div>
      </div>
      <div class="history-info">
        <div class="history-meta">${dist} · ${dur}${preset}</div>
        <div class="history-sub">${ago}</div>
      </div>
      <button class="history-del" onclick="event.stopPropagation();deleteFromHistory(${e.id})" title="Supprimer">✕</button>
    </div>`;
  }).join('');
}

function restoreHistory(id) {
  const entry = loadHistory().find(e => e.id === id);
  if (!entry) return;
  setRoutePoint('start', { lat: entry.sLat, lng: entry.sLng });
  setRoutePoint('end',   { lat: entry.eLat, lng: entry.eLng });
  if (entry.preset) setPreset(entry.preset);
  setTimeout(calculateRoute, 100);
}

function timeAgo(ts) {
  const diff = Math.floor((Date.now() - ts) / 1000);
  if (diff < 60)   return 'à l\'instant';
  if (diff < 3600) return `il y a ${Math.floor(diff/60)} min`;
  if (diff < 86400) return `il y a ${Math.floor(diff/3600)} h`;
  return `il y a ${Math.floor(diff/86400)} j`;
}

// ─── URL SHARING ────────────────────────────────────────────
function updateRouteHash() {
  const parts = [];
  if (routeStart) parts.push(`s=${routeStart.lat.toFixed(5)},${routeStart.lng.toFixed(5)}`);
  if (routeEnd)   parts.push(`e=${routeEnd.lat.toFixed(5)},${routeEnd.lng.toFixed(5)}`);
  if (rangePreset !== 'standard') parts.push(`p=${rangePreset}`);
  history.replaceState(null, '', parts.length ? '#' + parts.join('&') : location.pathname + location.search);
}

function copyRouteLink() {
  if (!routeStart && !routeEnd) { showToast('⚠ Aucun trajet à partager'); return; }
  updateRouteHash();
  const url = location.href;
  if (navigator.clipboard) {
    navigator.clipboard.writeText(url).then(() => showToast('✓ Lien copié dans le presse-papier'));
  } else {
    const el = Object.assign(document.createElement('input'), { value: url });
    document.body.appendChild(el); el.select(); document.execCommand('copy'); el.remove();
    showToast('✓ Lien copié');
  }
}

function restoreFromHash() {
  if (!location.hash) return;
  const params = Object.fromEntries(
    location.hash.slice(1).split('&').map(p => p.split('=').map(decodeURIComponent))
  );
  let hasStart = false, hasEnd = false;
  if (params.s) {
    const [lat, lng] = params.s.split(',').map(Number);
    if (!isNaN(lat) && !isNaN(lng)) { setRoutePoint('start', { lat, lng }); hasStart = true; }
  }
  if (params.e) {
    const [lat, lng] = params.e.split(',').map(Number);
    if (!isNaN(lat) && !isNaN(lng)) { setRoutePoint('end', { lat, lng }); hasEnd = true; }
  }
  if (params.p && ['conservative', 'standard', 'high'].includes(params.p)) setPreset(params.p);
  if (hasStart && hasEnd) setTimeout(calculateRoute, 600);
}

// ─── ROUTING (via backend /api/route) ───────────────────────
// Le frontend délègue tout au backend Rust qui appelle ORS.
// Avantages : la clé ORS reste dans .env (jamais exposée au navigateur),
// pas de problème CORS (l'appel ORS est serveur→serveur).

let routeStart = null, routeEnd = null;
let routePickMode = null; // 'start' | 'end' | null
let startMarker = null, endMarker = null;
let routeLayers = [];

// Draw polyline on map with glow
function drawRouteLayer(coords, color, dashArray, opacity = 0.9) {
  const glow = L.polyline(coords, { color, weight: 14, opacity: 0.1, lineJoin: 'round' }).addTo(map);
  const line = L.polyline(coords, { color, weight: 3.5, opacity, dashArray, lineJoin: 'round', lineCap: 'round' }).addTo(map);
  routeLayers.push(glow, line);
  return line;
}

// Dessine la route avec coloration vert/rouge par segment selon l'exposition.
// `segments` : Vec<bool> retourné par le backend (longueur = coords.length - 1).
// `safeColor` : couleur des segments non exposés (vert par défaut).
function drawSegmentedRoute(coords, segments, safeColor) {
  if (!segments || segments.length !== coords.length - 1) {
    // Fallback : route uniforme si données manquantes
    drawRouteLayer(coords, safeColor, null);
    return;
  }

  // Grouper les segments consécutifs de même statut en runs
  let runStart = 0;
  for (let i = 1; i <= segments.length; i++) {
    const endOfRun = (i === segments.length) || (segments[i] !== segments[runStart]);
    if (endOfRun) {
      const pts = coords.slice(runStart, i + 1); // +1 : inclure le point d'arrivée du run
      const exposed = segments[runStart];
      drawRouteLayer(pts, exposed ? '#ff3131' : safeColor, null, 0.9);
      runStart = i;
    }
  }
}

// Format helpers
function fmtDist(km) { return km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(2)} km`; }
function fmtTime(sec) {
  const m = Math.ceil(sec / 60);
  return m < 60 ? `~${m} min` : `~${Math.floor(m/60)}h${m % 60 > 0 ? (m%60)+'m' : ''}`;
}

// A/B map markers
function createRouteMarker(lat, lng, label, color) {
  const icon = L.divIcon({
    html: `<div style="width:26px;height:32px;position:relative;filter:drop-shadow(0 0 8px ${color});">
      <svg width="26" height="32" viewBox="0 0 26 32" fill="none" xmlns="http://www.w3.org/2000/svg">
        <path d="M13 0C5.82 0 0 5.82 0 13c0 9.75 13 19 13 19S26 22.75 26 13C26 5.82 20.18 0 13 0z" fill="${color}"/>
      </svg>
      <span style="position:absolute;top:6px;left:50%;transform:translateX(-50%);font-family:'Oxanium',sans-serif;font-weight:700;font-size:13px;color:#050a06;">${label}</span>
    </div>`,
    iconSize: [26, 32], iconAnchor: [13, 32], className: ''
  });
  return L.marker([lat, lng], { icon, zIndexOffset: 1000 });
}

function setRoutePoint(type, latlng) {
  const lat = latlng.lat, lng = latlng.lng;
  if (type === 'start') {
    routeStart = { lat, lng };
    if (startMarker) map.removeLayer(startMarker);
    startMarker = createRouteMarker(lat, lng, 'A', '#00ff41').addTo(map);
    document.getElementById('start-coord').textContent = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('rb-start').textContent = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('btn-set-start').classList.remove('active');
  } else {
    routeEnd = { lat, lng };
    if (endMarker) map.removeLayer(endMarker);
    endMarker = createRouteMarker(lat, lng, 'B', '#ff3131').addTo(map);
    document.getElementById('end-coord').textContent = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('rb-end').textContent = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('btn-set-end').classList.remove('active');
  }
  routePickMode = null;
  map.getContainer().style.cursor = '';
  modeBadge.classList.remove('active');
  updateRouteHash();
  openMobilePanel(); // revenir au panel après avoir placé le point
  // Show route bar if at least one point is set
  document.getElementById('route-bar').style.display = (routeStart || routeEnd) ? 'flex' : 'none';
}

function clearPickButtons() {
  document.getElementById('btn-set-start').classList.remove('active');
  document.getElementById('btn-set-end').classList.remove('active');
}

function clearRoute() {
  routeLayers.forEach(l => map.removeLayer(l)); routeLayers = [];
  if (startMarker) { map.removeLayer(startMarker); startMarker = null; }
  if (endMarker) { map.removeLayer(endMarker); endMarker = null; }
  routeStart = null; routeEnd = null; routePickMode = null;
  updateRouteHash();
  document.getElementById('start-coord').textContent = 'Non défini';
  document.getElementById('end-coord').textContent = 'Non défini';
  document.getElementById('rb-start').textContent = 'Non défini';
  document.getElementById('rb-end').textContent = 'Non défini';
  document.getElementById('route-result').style.display = 'none';
  document.getElementById('route-clear-btn').style.display = 'none';
  document.getElementById('route-bar').style.display = 'none';
  document.getElementById('exposure-section').style.display = 'none';
  const gpxBtn = document.getElementById('btn-gpx');
  if (gpxBtn.href.startsWith('blob:')) URL.revokeObjectURL(gpxBtn.href);
  gpxBtn.href = '#';
  document.getElementById('nav-apps').style.display = 'none';
  map.getContainer().style.cursor = '';
  modeBadge.classList.remove('active');
}

// Main route calculation — délégué au backend Rust
async function calculateRoute() {
  if (!routeStart || !routeEnd) {
    showToast('⚠ Définir le départ (A) et l\'arrivée (B) d\'abord');
    return;
  }

  routeLayers.forEach(l => map.removeLayer(l)); routeLayers = [];

  const btn = document.getElementById('route-btn');
  btn.disabled = true; btn.textContent = 'CALCUL EN COURS...';
  document.getElementById('route-result').style.display = 'none';

  try {
    const avoidCams = document.getElementById('avoid-cams').checked;
    const showComp  = document.getElementById('show-comparison').checked;

    const res = await fetch('/api/route', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        start:          routeStart,
        end:            routeEnd,
        avoid_cams:     avoidCams,
        include_direct: avoidCams && showComp,
        range_preset:   rangePreset,   // "conservative" | "standard" | "high"
      }),
      signal: AbortSignal.timeout(20000),
    });

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || `HTTP ${res.status}`);
    }

    const data = await res.json();

    // GeoJSON [lng, lat] → Leaflet [lat, lng]
    const coords = data.route.coordinates.map(([lo, la]) => [la, lo]);

    // Route colorée par segment (vert = sûr, rouge = exposé)
    if (avoidCams) {
      drawSegmentedRoute(coords, data.segments, '#00ff41');
    } else {
      drawRouteLayer(coords, '#00b8d4', null);
    }

    document.getElementById('route-label-safe').textContent = avoidCams ? 'ROUTE SÛRE' : 'ROUTE CALCULÉE';
    document.getElementById('route-dist-safe').textContent = fmtDist(data.distance_km);
    document.getElementById('route-time-safe').textContent = fmtTime(data.duration_sec);

    // Score d'exposition — utilise les segments backend (précis, toutes caméras de la bbox)
    // Fallback : calcul côté client si segments indisponibles
    let score;
    if (avoidCams && data.segments && data.segments.length === coords.length - 1) {
      let exposedM = 0, totalM = 0;
      for (let i = 0; i < data.segments.length; i++) {
        const d = haversineDistance(coords[i][0], coords[i][1], coords[i+1][0], coords[i+1][1]);
        totalM += d;
        if (data.segments[i]) exposedM += d;
      }
      score = {
        pct:      totalM > 0 ? Math.round(exposedM / totalM * 100) : 0,
        exposedM: Math.round(exposedM),
        totalM:   Math.round(totalM),
      };
    } else {
      score = computeExposureScore(coords);
    }
    displayExposureScore(score, avoidCams);

    // Compteur de caméras évitées (donné par le backend)
    document.getElementById('route-cams-count').textContent =
      avoidCams && data.cams_avoided > 0
        ? `${data.cams_avoided} caméra${data.cams_avoided > 1 ? 's' : ''} dans la zone`
        : '';

    // Avertissement si les zones ont dû être réduites (ORS 2010 → retry ×0.5)
    const relaxedBanner = document.getElementById('route-relaxed-warn');
    if (relaxedBanner) {
      relaxedBanner.style.display = data.relaxed ? 'block' : 'none';
    }

    // Route directe (optionnelle)
    document.getElementById('direct-route-row').style.display = 'none';
    if (data.direct_route) {
      const coords2 = data.direct_route.route.coordinates.map(([lo, la]) => [la, lo]);
      drawRouteLayer(coords2, '#ffb300', '8,5', 0.6);
      document.getElementById('route-dist-direct').textContent = fmtDist(data.direct_route.distance_km);
      document.getElementById('route-time-direct').textContent = fmtTime(data.direct_route.duration_sec);
      document.getElementById('direct-route-row').style.display = 'flex';
    }

    document.getElementById('route-result').style.display = 'block';
    document.getElementById('route-clear-btn').style.display = 'block';

    // Génération GPX depuis les coordonnées ORS [lng, lat]
    const trkpts = data.route.coordinates
      .map(([lo, la]) => `      <trkpt lat="${la.toFixed(6)}" lon="${lo.toFixed(6)}"></trkpt>`)
      .join('\n');
    const gpx = `<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="BlindSpot MTL" xmlns="http://www.topografix.com/GPX/1/1">
  <trk>
    <name>BlindSpot Route</name>
    <trkseg>
${trkpts}
    </trkseg>
  </trk>
</gpx>`;
    const blob = new Blob([gpx], { type: 'application/gpx+xml' });
    document.getElementById('btn-gpx').href = URL.createObjectURL(blob);
    document.getElementById('nav-apps').style.display = 'block';

    // Sauvegarder dans l'historique
    saveToHistory({
      sLat: routeStart.lat, sLng: routeStart.lng,
      eLat: routeEnd.lat,   eLng: routeEnd.lng,
      preset: rangePreset,
      distKm: data.distance_km,
      durSec: data.duration_sec,
    });

    openMobilePanel(); // afficher les résultats sur mobile

    map.fitBounds(L.polyline(coords).getBounds(), { padding: [60, 60] });
    const toastMsg = avoidCams
      ? (data.relaxed
          ? `⚠ Route partiellement sûre (zones réduites) : ${fmtDist(data.distance_km)}`
          : `✓ Route sûre : ${fmtDist(data.distance_km)} — ${data.cams_avoided} caméra(s) évitée(s)`)
      : `✓ Route : ${fmtDist(data.distance_km)}`;
    showToast(toastMsg);

  } catch (err) {
    console.error('Routing error:', err);
    showToast(`⚠ Erreur routing : ${err.message}`);
  } finally {
    btn.disabled = false; btn.textContent = '▶ CALCULER L\'ITINÉRAIRE';
  }
}

// ─── PICK BUTTONS ───────────────────────────────────────────
document.getElementById('btn-set-start').addEventListener('click', () => {
  addMode = false;
  btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
  if (routePickMode === 'start') {
    routePickMode = null; clearPickButtons();
    map.getContainer().style.cursor = ''; modeBadge.classList.remove('active'); return;
  }
  routePickMode = 'start'; clearPickButtons();
  document.getElementById('btn-set-start').classList.add('active');
  map.getContainer().style.cursor = 'crosshair';
  modeBadge.textContent = 'A DÉFINIR LE DÉPART'; modeBadge.classList.add('active');
  closeMobilePanel();
  showToast('Cliquer sur la carte pour définir le départ (A)');
});

document.getElementById('btn-set-end').addEventListener('click', () => {
  addMode = false;
  btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
  if (routePickMode === 'end') {
    routePickMode = null; clearPickButtons();
    map.getContainer().style.cursor = ''; modeBadge.classList.remove('active'); return;
  }
  routePickMode = 'end'; clearPickButtons();
  document.getElementById('btn-set-end').classList.add('active');
  map.getContainer().style.cursor = 'crosshair';
  modeBadge.textContent = 'B DÉFINIR L\'ARRIVÉE'; modeBadge.classList.add('active');
  closeMobilePanel();
  showToast('Cliquer sur la carte pour définir l\'arrivée (B)');
});

document.getElementById('route-btn').addEventListener('click', calculateRoute);
document.getElementById('route-clear-btn').addEventListener('click', clearRoute);

// ─── MAP CLICK (unified handler) ────────────────────────────
map.on('click', (e) => {
  if (routePickMode === 'start') {
    setRoutePoint('start', e.latlng);
  } else if (routePickMode === 'end') {
    setRoutePoint('end', e.latlng);
  } else if (addMode) {
    const typeVal = document.getElementById('cam-type').value;
    const dirVal = parseFloat(document.getElementById('cam-direction').value) || 0;
    const fovVal = parseFloat(document.getElementById('cam-fov').value) || 70;
    const rangeVal = parseFloat(document.getElementById('cam-range').value) || 30;
    const noteVal = document.getElementById('cam-note').value.trim();
    const cam = {
      lat: e.latlng.lat, lng: e.latlng.lng,
      direction: typeVal === 'ptz' || typeVal === 'unknown' ? null : dirVal,
      fov: fovVal, range: rangeVal, type: typeVal,
      name: noteVal || 'Caméra communautaire', source: 'user', note: noteVal || null
    };
    cameras.push(cam);
    const newIdx = cameras.length - 1;
    mountCamera(cam, newIdx);
    persistCamera(cam); // async — fire & forget (localStorage ou backend)
    userCameraCount++; updateStats(); updateList();
    addMode = false;
    stopOrientTracking();
    map.getContainer().style.cursor = ''; modeBadge.classList.remove('active');
    btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
    document.getElementById('cam-note').value = '';
    showToast('✓ Caméra ajoutée');
  }
});

// ─── DEBOUNCE ───────────────────────────────────────────────
/** Retarde l'exécution de fn jusqu'à ce que les appels s'arrêtent pendant `ms` ms. */
function debounce(fn, ms) {
  let timer;
  return (...args) => { clearTimeout(timer); timer = setTimeout(() => fn(...args), ms); };
}

const syncViewportDebounced = debounce(syncViewport, 120);

// ─── BOOT ───────────────────────────────────────────────────
setTimeout(() => map.invalidateSize(), 100);

map.on('moveend', () => {
  syncViewportDebounced();
  const bounds = map.getBounds().pad(0.4);
  const s = bounds.getSouth(), w = bounds.getWest();
  const n = bounds.getNorth(), e = bounds.getEast();
  if (viewportNeedsLoad(bounds)) loadCamerasForBbox(s, w, n, e);
  // Bâtiments pour le LOS — même si les caméras sont déjà chargées
  else loadBuildingsForBbox(s, w, n, e);
});

map.on('zoomend', syncViewportDebounced);

// ─── JUMP TO CAMERA (admin link ?lat=&lng=&z=&cam=) ─────────
let _highlightCamId = null;

function restoreFromQuery() {
  const p = new URLSearchParams(location.search);
  const lat = parseFloat(p.get('lat'));
  const lng = parseFloat(p.get('lng'));
  const z   = parseInt(p.get('z')) || 18;
  const cam = parseInt(p.get('cam'));

  if (isNaN(lat) || isNaN(lng)) return;

  map.setView([lat, lng], z);

  // Anneau pulsant pour indiquer la cible
  const ring = L.circleMarker([lat, lng], {
    radius: 20, color: '#00ff41', fill: false, weight: 2, opacity: 1,
  }).addTo(map);
  let op = 1;
  const fade = setInterval(() => {
    op -= 0.04;
    ring.setStyle({ opacity: Math.max(0, op) });
    if (op <= 0) { clearInterval(fade); map.removeLayer(ring); }
  }, 80);

  if (!isNaN(cam)) _highlightCamId = cam;
}

function _tryOpenHighlight() {
  if (_highlightCamId === null) return;
  const idx = cameras.findIndex(c => c.id === _highlightCamId);
  if (idx === -1) return;
  const entry = renderedCameras.get(idx);
  if (!entry) return;
  entry.marker.openPopup();
  _highlightCamId = null;
}

document.getElementById('geoloc-btn').addEventListener('click', locateUser);
restoreFromQuery(); // doit être avant loadCameras() pour que le viewport soit correct
loadCameras();
renderHistory();
restoreFromHash();

// ─── SIDEBAR RESIZE (desktop) ────────────────────────────────
(function () {
  const sidebar = document.getElementById('sidebar');
  const handle  = document.getElementById('sidebar-resize');
  const KEY     = 'blindspot_sidebar_w';

  // Restaurer la largeur sauvegardée
  const saved = parseInt(localStorage.getItem(KEY));
  if (saved && window.innerWidth > 768) sidebar.style.width = saved + 'px';

  handle.addEventListener('mousedown', (e) => {
    if (window.innerWidth <= 768) return;
    e.preventDefault();
    handle.classList.add('dragging');
    const startX = e.clientX;
    const startW = sidebar.getBoundingClientRect().width;

    function onMove(e) {
      // La sidebar est à droite : tirer vers la gauche = agrandir
      const w = Math.min(520, Math.max(220, startW + (startX - e.clientX)));
      sidebar.style.width = w + 'px';
    }
    function onUp() {
      handle.classList.remove('dragging');
      localStorage.setItem(KEY, parseInt(sidebar.style.width));
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup',   onUp);
    }
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup',   onUp);
  });
})();

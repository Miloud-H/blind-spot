// ─── CAMERAS — État, rendu, chargement ───────────────────────────────────────

let cameras           = [];
const renderedCameras = new Map();
let userCameraCount   = 0;
const cameraIdSet     = new Set();

// ── Tile cache (0.01° ≈ 1 km) ──
const TILE_DEG   = 0.01;
const loadedTiles = new Set();

function _tileKey(tx, ty)  { return `${tx},${ty}`; }

function _markAndGetFetchBbox(s, w, n, e) {
  const txMin = Math.floor(w / TILE_DEG), txMax = Math.floor(e / TILE_DEG);
  const tyMin = Math.floor(s / TILE_DEG), tyMax = Math.floor(n / TILE_DEG);
  const missing = [];
  for (let ty = tyMin; ty <= tyMax; ty++)
    for (let tx = txMin; tx <= txMax; tx++)
      if (!loadedTiles.has(_tileKey(tx, ty))) missing.push([tx, ty]);
  if (!missing.length) return null;
  missing.forEach(([tx, ty]) => loadedTiles.add(_tileKey(tx, ty)));
  const fs = Math.min(...missing.map(([, ty]) => ty))       * TILE_DEG;
  const fw = Math.min(...missing.map(([tx])    => tx))       * TILE_DEG;
  const fn_ = (Math.max(...missing.map(([, ty]) => ty)) + 1) * TILE_DEG;
  const fe  = (Math.max(...missing.map(([tx])  => tx))  + 1) * TILE_DEG;
  return [fs, fw, fn_, fe];
}

const BASE_RANGE  = { fixed: 38, ptz: 28, unknown: 20 };
const PRESET_MULT = { conservative: 0.5, standard: 1.0, high: 2.2 };
let rangePreset = 'standard';

function getRange(cam) {
  if (cam.source === 'user') return (cam.range || 30) * PRESET_MULT[rangePreset];
  return (BASE_RANGE[cam.type] ?? BASE_RANGE.unknown) * PRESET_MULT[rangePreset];
}

function isPointInCameraZone(lat, lng, cam) {
  const isPTZ  = cam.type === 'ptz' || cam.type === 'dome';
  const base   = cam.source === 'user' ? (cam.range || 30) : (BASE_RANGE[cam.type] ?? BASE_RANGE.unknown);
  const rangeM = base * PRESET_MULT.standard;
  const dist   = haversineDistance(lat, lng, cam.lat, cam.lng);
  if (dist > rangeM * 1.15) return false;
  if (isPTZ || cam.direction === null) return dist <= rangeM;
  const bearing = bearingTo(cam.lat, cam.lng, lat, lng);
  let diff = Math.abs(bearing - cam.direction);
  if (diff > 180) diff = 360 - diff;
  return diff <= (cam.fov || 70) / 2 && dist <= rangeM;
}

function computeExposureScore(coords) {
  let exposedM = 0, totalM = 0;
  for (let i = 0; i < coords.length - 1; i++) {
    const [la, lo] = coords[i];
    const segM = haversineDistance(la, lo, coords[i+1][0], coords[i+1][1]);
    totalM += segM;
    if (cameras.some(cam => isPointInCameraZone(la, lo, cam))) exposedM += segM;
  }
  return {
    pct:      totalM > 0 ? Math.round(exposedM / totalM * 100) : 0,
    exposedM: Math.round(exposedM),
    totalM:   Math.round(totalM),
  };
}

// ── Rendu ──

const ZONE_STYLES = {
  fixed: {
    conservative: { fill: 'rgba(255,40,40,0.38)',  stroke: 'rgba(255,40,40,0.88)',  w: 1.5, dash: null  },
    standard:     { fill: 'rgba(255,165,0,0.28)',  stroke: 'rgba(255,165,0,0.80)',  w: 1.2, dash: null  },
    high:         { fill: 'rgba(50,210,50,0.20)',  stroke: 'rgba(50,210,50,0.70)',  w: 1.0, dash: '4,6' },
  },
  ptz: {
    conservative: { fill: 'rgba(255,40,40,0.34)',  stroke: 'rgba(255,40,40,0.82)',  w: 1.4, dash: null  },
    standard:     { fill: 'rgba(255,165,0,0.25)',  stroke: 'rgba(255,165,0,0.73)',  w: 1.2, dash: null  },
    high:         { fill: 'rgba(50,210,50,0.17)',  stroke: 'rgba(50,210,50,0.62)',  w: 1.0, dash: '4,6' },
  },
};

function renderCamera(cam) {
  const { id, lat, lng, direction, fov, type, name, source, note } = cam;
  const isPTZ  = type === 'ptz' || type === 'dome';
  const hasDir = direction !== null && !isPTZ;
  const styles = isPTZ ? ZONE_STYLES.ptz : ZONE_STYLES.fixed;
  const z      = styles[rangePreset] ?? styles.standard;
  const rangeM = getRange(cam);
  const opts   = { fillColor: z.fill, fillOpacity: 1, color: z.stroke, weight: z.w, dashArray: z.dash };

  let poly;
  if (buildings.length > 0) {
    const vDir  = hasDir ? direction : null;
    const vFov  = hasDir ? (fov || 70) : 360;
    const nRays = isPTZ ? 180 : (hasDir ? Math.max(60, Math.round(vFov)) : 120);
    poly = L.polygon(computeViewshed(lat, lng, rangeM, vDir, vFov, nRays), opts);
  } else if (isPTZ || !hasDir) {
    poly = L.polygon(buildCircle(lat, lng, rangeM, 36), opts);
  } else {
    poly = L.polygon(buildCone(lat, lng, direction, fov || 70, rangeM, 24), opts);
  }
  const zoneLayers = [poly];

  const isInferred = source === 'inferred';
  const dotColor = source === 'user' ? '#ffb300'
                 : isInferred        ? '#00b8d4'
                 : isPTZ             ? '#ff8c00'
                 :                     '#ff3131';
  const camIcon = L.divIcon({
    html: `<div style="width:8px;height:8px;background:${dotColor};border-radius:50%;border:1px solid rgba(255,255,255,0.35);box-shadow:0 0 6px ${dotColor};"></div>`,
    iconSize: [8,8], iconAnchor: [4,4], className: '',
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
      source === 'user' ? '👤 Communauté' : source === 'inferred' ? '🔍 Déduite' : '🗺 OSM'
    }</span></div>
    ${note ? `<div class="popup-row">Note: <span>${note}</span></div>` : ''}
    <div class="popup-row" style="margin-top:6px;font-size:10px;color:var(--text-dim)">${lat.toFixed(5)}, ${lng.toFixed(5)}</div>
    <button onclick="reportCamera(${id})" style="margin-top:8px;width:100%;padding:4px 8px;background:transparent;border:1px solid rgba(255,49,49,0.35);color:var(--red);font-size:9px;letter-spacing:1px;cursor:pointer;font-family:inherit;">⚠ SIGNALER COMME RETIRÉE</button>`;
  marker.bindPopup(popupHtml, { maxWidth: 220 });
  zoneLayers.forEach(p => p.on('click', () => marker.openPopup()));
  return { zones: zoneLayers, marker };
}

function mountCamera(cam, idx) {
  if (renderedCameras.has(idx)) return;
  const { zones, marker } = renderCamera(cam);
  zones.forEach(z => z.addTo(map));
  marker.addTo(map);
  renderedCameras.set(idx, { zones, marker });
}

function unmountCamera(idx) {
  const entry = renderedCameras.get(idx);
  if (!entry) return;
  entry.zones.forEach(l => map.removeLayer(l));
  map.removeLayer(entry.marker);
  renderedCameras.delete(idx);
}

function syncViewport() {
  const bounds = map.getBounds().pad(0.35);
  for (const idx of [...renderedCameras.keys()]) {
    const cam = cameras[idx];
    if (cam && !bounds.contains([cam.lat, cam.lng])) unmountCamera(idx);
  }
  cameras.forEach((cam, idx) => {
    if (!renderedCameras.has(idx) && bounds.contains([cam.lat, cam.lng])) mountCamera(cam, idx);
  });
}

// ── Liste + stats ──

function updateList() {
  const list = document.getElementById('camera-list');
  list.innerHTML = '';
  [...cameras].reverse().forEach((cam, i) => {
    const item = document.createElement('div');
    item.className = 'camera-item';
    const isPTZ = cam.type === 'ptz' || cam.type === 'dome';
    const icon  = isPTZ ? '🔄' : (cam.direction !== null ? '📹' : '📷');
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
  document.getElementById('stat-ptz').textContent   = cameras.filter(c => c.type === 'ptz' || c.type === 'dome').length;
  document.getElementById('stat-user').textContent  = userCameraCount;
  const el = document.getElementById('stat-inferred');
  if (el) el.textContent = cameras.filter(c => c.source === 'inferred').length;
}

// ── Chargement bâtiments (déclenche re-rendu LOS une fois prêt) ──

async function loadBuildingsForBbox(s, w, n, e) {
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
      for (const idx of [...renderedCameras.keys()]) unmountCamera(idx);
      syncViewport();
    }
  } catch (_) {}
}

// ── Chargement caméras (lazy, par viewport) ──

async function loadCamerasForBbox(s, w, n, e, isInitial = false) {
  const fetchBbox = _markAndGetFetchBbox(s, w, n, e);
  if (!fetchBbox) {
    if (isInitial) document.getElementById('loading').style.display = 'none';
    return;
  }
  const [fs, fw, fn_, fe] = fetchBbox;

  if (isInitial) {
    document.getElementById('dot-osm').className = 'dot loading';
    document.getElementById('status-osm').textContent = 'CHARGEMENT...';
  }
  try {
    const res = await fetch(`/api/cameras?bbox=${fs},${fw},${fn_},${fe}`, {
      signal: AbortSignal.timeout(15000),
    });
    if (isInitial) document.getElementById('loading').style.display = 'none';
    if (!res.ok) throw new Error(`HTTP ${res.status}`);

    const data = await res.json();
    let added = 0;
    data.forEach(c => {
      if (cameraIdSet.has(c.id)) return;
      cameraIdSet.add(c.id);
      cameras.push({
        id:        c.id,
        lat:       c.lat,
        lng:       c.lng,
        direction: c.direction,
        fov:       c.fov      ?? 70,
        range:     c.range_m  ?? 30,
        type:      c.cam_type ?? 'unknown',
        name:      c.name,
        source:    c.source,
        note:      c.note,
      });
      if (c.source === 'user') userCameraCount++;
      added++;
    });

    if (added > 0) { syncViewport(); updateStats(); updateList(); _tryOpenHighlight(); }

    loadBuildingsForBbox(s, w, n, e);

    if (isInitial) {
      const osmCount = cameras.filter(c => c.source === 'osm').length;
      document.getElementById('dot-osm').className = 'dot';
      document.getElementById('status-osm').textContent = `OSM: ${osmCount} CAMÉRAS`;
      if (osmCount === 0) showToast('⚠ Import OSM en cours — rafraîchir dans quelques secondes');
      else                showToast(`✓ ${osmCount} caméras chargées`);
    }
  } catch (e) {
    if (isInitial) {
      document.getElementById('loading').style.display = 'none';
      document.getElementById('dot-osm').className = 'dot error';
      document.getElementById('status-osm').textContent = 'BACKEND HORS-LIGNE';
      showToast('⚠ Backend inaccessible — données de démo');
      loadDemoData();
    }
  }
}

async function loadCameras() {
  const b = map.getBounds().pad(0.6);
  await loadCamerasForBbox(b.getSouth(), b.getWest(), b.getNorth(), b.getEast(), true);
}

function viewportNeedsLoad(bounds) {
  const txMin = Math.floor(bounds.getWest()  / TILE_DEG);
  const txMax = Math.floor(bounds.getEast()  / TILE_DEG);
  const tyMin = Math.floor(bounds.getSouth() / TILE_DEG);
  const tyMax = Math.floor(bounds.getNorth() / TILE_DEG);
  for (let ty = tyMin; ty <= tyMax; ty++)
    for (let tx = txMin; tx <= txMax; tx++)
      if (!loadedTiles.has(_tileKey(tx, ty))) return true;
  return false;
}

function loadDemoData() {
  [
    { lat: 45.5088, lng: -73.5540, direction: 220, fov: 70, range: 35, type: 'fixed',   name: 'SPVM - Ste-Catherine',    source: 'osm', note: 'Caméra de sécurité urbaine' },
    { lat: 45.5120, lng: -73.5610, direction: null,fov: 70, range: 25, type: 'ptz',     name: 'Dôme PTZ - Guy-Concordia',source: 'osm', note: null },
    { lat: 45.5060, lng: -73.5720, direction: 90,  fov: 80, range: 40, type: 'fixed',   name: 'Commerce - Atwater',      source: 'osm', note: null },
    { lat: 45.5230, lng: -73.5860, direction: 315, fov: 60, range: 30, type: 'fixed',   name: 'Caméra fixe',             source: 'osm', note: null },
    { lat: 45.5195, lng: -73.5780, direction: null,fov: 70, range: 20, type: 'unknown', name: 'Caméra inconnue',         source: 'osm', note: null },
    { lat: 45.5160, lng: -73.5650, direction: 180, fov: 90, range: 45, type: 'fixed',   name: 'Entrée parking',          source: 'osm', note: null },
  ].forEach(c => cameras.push(c));
  syncViewport(); updateStats(); updateList();
  showToast('Mode démo — données simulées pour Montréal');
}

// ── Persistance ──

async function persistCamera(cam) {
  try {
    const res = await fetch('/api/cameras', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        lat: cam.lat, lng: cam.lng, direction: cam.direction,
        fov: cam.fov, range_m: cam.range, cam_type: cam.type,
        name: cam.name, note: cam.note,
      }),
      signal: AbortSignal.timeout(5000),
    });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
  } catch (e) {
    try {
      const stored = JSON.parse(localStorage.getItem('blindspot_community_cameras') || '[]');
      stored.push({ ...cam });
      localStorage.setItem('blindspot_community_cameras', JSON.stringify(stored));
    } catch {}
  }
}

async function reportCamera(id) {
  try {
    const res = await fetch(`/api/cameras/${id}/report`, { method: 'POST' });
    if (res.ok) showToast('✓ Signalement enregistré — merci');
    else        showToast('⚠ Erreur lors du signalement');
  } catch { showToast('⚠ Erreur réseau'); }
}

// ── WebSocket — mises à jour temps réel ──

function handleWsEvent(ev) {
  if (ev.type === 'camera_added') {
    const c = ev.camera;
    if (cameraIdSet.has(c.id)) return;
    cameraIdSet.add(c.id);
    const cam = {
      id: c.id, lat: c.lat, lng: c.lng,
      direction: c.direction,
      fov:    c.fov    ?? 70,
      range:  c.range_m ?? 30,
      type:   c.cam_type ?? 'unknown',
      name:   c.name,
      source: c.source ?? 'user',
      note:   c.note,
    };
    if (cam.source === 'user') userCameraCount++;
    cameras.push(cam);
    const idx = cameras.length - 1;
    if (map.getBounds().pad(0.35).contains([cam.lat, cam.lng])) mountCamera(cam, idx);
    updateStats(); updateList();

  } else if (ev.type === 'camera_deleted') {
    const idx = cameras.findIndex(c => c.id === ev.id);
    if (idx === -1) return;
    if (cameras[idx].source === 'user') userCameraCount--;
    for (const k of [...renderedCameras.keys()]) unmountCamera(k);
    cameras.splice(idx, 1);
    cameraIdSet.delete(ev.id);
    syncViewport();
    updateStats(); updateList();

  } else if (ev.type === 'camera_updated') {
    const idx = cameras.findIndex(c => c.id === ev.id);
    if (idx === -1) return;
    cameras[idx] = {
      ...cameras[idx],
      lat: ev.lat, lng: ev.lng,
      direction: ev.direction,
      fov:    ev.fov    ?? cameras[idx].fov,
      range:  ev.range_m ?? cameras[idx].range,
      type:   ev.cam_type ?? cameras[idx].type,
      name:   ev.name  ?? cameras[idx].name,
      note:   ev.note  ?? cameras[idx].note,
    };
    unmountCamera(idx);
    if (map.getBounds().pad(0.35).contains([cameras[idx].lat, cameras[idx].lng])) {
      mountCamera(cameras[idx], idx);
    }
    updateStats(); updateList();
  }
}

function connectEventStream(delay = 1000) {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/api/events`);
  ws.onopen    = () => { delay = 1000; };
  ws.onmessage = e => { try { handleWsEvent(JSON.parse(e.data)); } catch {} };
  ws.onclose   = () => setTimeout(() => connectEventStream(Math.min(delay * 2, 30000)), delay);
}

// ── Jump-to-camera (lien admin ?cam=) ──

let _highlightCamId = null;

function _tryOpenHighlight() {
  if (_highlightCamId === null) return;
  const idx = cameras.findIndex(c => c.id === _highlightCamId);
  if (idx === -1) return;
  const entry = renderedCameras.get(idx);
  if (!entry) return;
  entry.marker.openPopup();
  _highlightCamId = null;
}

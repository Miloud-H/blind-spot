// ─── ROUTING — État, calcul, géocodage, historique, URL ─────────────────────

let routeStart    = null, routeEnd = null;
let routePickMode = null;
let startMarker   = null, endMarker = null;
let routeLayers   = [];

// ── Helpers affichage ──

function fmtDist(km) { return km < 1 ? `${Math.round(km * 1000)} m` : `${km.toFixed(2)} km`; }
function fmtTime(sec) {
  const m = Math.ceil(sec / 60);
  return m < 60 ? `~${m} min` : `~${Math.floor(m/60)}h${m % 60 > 0 ? (m%60)+'m' : ''}`;
}

function displayExposureScore(score, avoidMode) {
  const sec = document.getElementById('exposure-section');
  if (!avoidMode || score.totalM === 0) { sec.style.display = 'none'; return; }
  const pct     = score.pct;
  const color   = pct < 10 ? 'var(--green)' : pct < 35 ? 'var(--amber)' : 'var(--red)';
  const verdict = pct < 10 ? '✓ Itinéraire sûr' : pct < 35 ? '⚠ Partiellement exposé' : '⚠ Fortement exposé';
  document.getElementById('exposure-fill').style.width      = `${pct}%`;
  document.getElementById('exposure-fill').style.background = color;
  document.getElementById('exposure-pct').textContent       = `${pct}%`;
  document.getElementById('exposure-pct').style.color       = color;
  document.getElementById('exposure-detail').textContent    = `${score.exposedM} m exposés / ${score.totalM} m`;
  const v = document.getElementById('exposure-verdict');
  v.textContent = verdict; v.style.color = color;
  sec.style.display = 'block';
}

// ── Dessin de route ──

function drawRouteLayer(coords, color, dashArray, opacity = 0.9) {
  const glow = L.polyline(coords, { color, weight: 14, opacity: 0.1, lineJoin: 'round' }).addTo(map);
  const line = L.polyline(coords, { color, weight: 3.5, opacity, dashArray, lineJoin: 'round', lineCap: 'round' }).addTo(map);
  routeLayers.push(glow, line);
  return line;
}

function drawSegmentedRoute(coords, segments, safeColor) {
  if (!segments || segments.length !== coords.length - 1) {
    drawRouteLayer(coords, safeColor, null);
    return;
  }
  let runStart = 0;
  for (let i = 1; i <= segments.length; i++) {
    if (i === segments.length || segments[i] !== segments[runStart]) {
      drawRouteLayer(coords.slice(runStart, i + 1), segments[runStart] ? '#ff3131' : safeColor, null, 0.9);
      runStart = i;
    }
  }
}

// ── Marqueurs A / B ──

function createRouteMarker(lat, lng, label, color) {
  const icon = L.divIcon({
    html: `<div style="width:26px;height:32px;position:relative;filter:drop-shadow(0 0 8px ${color});">
      <svg width="26" height="32" viewBox="0 0 26 32" fill="none">
        <path d="M13 0C5.82 0 0 5.82 0 13c0 9.75 13 19 13 19S26 22.75 26 13C26 5.82 20.18 0 13 0z" fill="${color}"/>
      </svg>
      <span style="position:absolute;top:6px;left:50%;transform:translateX(-50%);font-family:'Oxanium',sans-serif;font-weight:700;font-size:13px;color:#050a06;">${label}</span>
    </div>`,
    iconSize: [26, 32], iconAnchor: [13, 32], className: '',
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
    document.getElementById('rb-start').textContent    = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('btn-set-start').classList.remove('active');
  } else {
    routeEnd = { lat, lng };
    if (endMarker) map.removeLayer(endMarker);
    endMarker = createRouteMarker(lat, lng, 'B', '#ff3131').addTo(map);
    document.getElementById('end-coord').textContent = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('rb-end').textContent    = `${lat.toFixed(4)}, ${lng.toFixed(4)}`;
    document.getElementById('btn-set-end').classList.remove('active');
  }
  routePickMode = null;
  map.getContainer().style.cursor = '';
  modeBadge.classList.remove('active');
  updateRouteHash();
  openMobilePanel();
  document.getElementById('route-bar').style.display = (routeStart || routeEnd) ? 'flex' : 'none';
}

function clearPickButtons() {
  document.getElementById('btn-set-start').classList.remove('active');
  document.getElementById('btn-set-end').classList.remove('active');
}

function clearRoute() {
  routeLayers.forEach(l => map.removeLayer(l)); routeLayers = [];
  if (startMarker) { map.removeLayer(startMarker); startMarker = null; }
  if (endMarker)   { map.removeLayer(endMarker);   endMarker   = null; }
  routeStart = null; routeEnd = null; routePickMode = null;
  updateRouteHash();
  document.getElementById('start-coord').textContent       = 'Non défini';
  document.getElementById('end-coord').textContent         = 'Non défini';
  document.getElementById('rb-start').textContent          = 'Non défini';
  document.getElementById('rb-end').textContent            = 'Non défini';
  document.getElementById('route-result').style.display    = 'none';
  document.getElementById('route-clear-btn').style.display = 'none';
  document.getElementById('route-bar').style.display       = 'none';
  document.getElementById('exposure-section').style.display = 'none';
  const gpxBtn = document.getElementById('btn-gpx');
  if (gpxBtn.href.startsWith('blob:')) URL.revokeObjectURL(gpxBtn.href);
  gpxBtn.href = '#';
  document.getElementById('nav-apps').style.display = 'none';
  map.getContainer().style.cursor = '';
  modeBadge.classList.remove('active');
}

// ── Géocodage adresse (Nominatim) ──

async function geocodeAddr(type) {
  const inputEl = document.getElementById(`addr-${type}`);
  const q = inputEl.value.trim();
  if (!q) return;
  inputEl.disabled = true;
  try {
    // viewbox = soft bias vers Montréal (bounded=0 : pas de restriction stricte)
    const url = `https://nominatim.openstreetmap.org/search?q=${encodeURIComponent(q)}&format=json&limit=1&countrycodes=ca&viewbox=-74.1,45.75,-73.2,45.35&bounded=0`;
    const r = await fetch(url, { signal: AbortSignal.timeout(8000) });
    const results = await r.json();
    if (!results.length) { showToast('⚠ Adresse introuvable'); return; }
    const { lat, lon, display_name } = results[0];
    const latlng = { lat: parseFloat(lat), lng: parseFloat(lon) };
    setRoutePoint(type, latlng);
    map.setView([latlng.lat, latlng.lng], 17);
    inputEl.value = display_name.split(',').slice(0, 2).join(', ');
  } catch {
    showToast('⚠ Erreur de géocodage');
  } finally {
    inputEl.disabled = false;
  }
}

// ── Calcul de route ──

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
        range_preset:   rangePreset,
      }),
      signal: AbortSignal.timeout(20000),
    });

    if (!res.ok) {
      const err = await res.json().catch(() => ({}));
      throw new Error(err.error || `HTTP ${res.status}`);
    }

    const data   = await res.json();
    const coords = data.route.coordinates.map(([lo, la]) => [la, lo]);

    if (avoidCams) drawSegmentedRoute(coords, data.segments, '#00ff41');
    else           drawRouteLayer(coords, '#00b8d4', null);

    document.getElementById('route-label-safe').textContent = avoidCams ? 'ROUTE SÛRE' : 'ROUTE CALCULÉE';
    document.getElementById('route-dist-safe').textContent  = fmtDist(data.distance_km);
    document.getElementById('route-time-safe').textContent  = fmtTime(data.duration_sec);

    let score;
    if (avoidCams && data.segments && data.segments.length === coords.length - 1) {
      let exposedM = 0, totalM = 0;
      for (let i = 0; i < data.segments.length; i++) {
        const d = haversineDistance(coords[i][0], coords[i][1], coords[i+1][0], coords[i+1][1]);
        totalM += d;
        if (data.segments[i]) exposedM += d;
      }
      score = { pct: totalM > 0 ? Math.round(exposedM / totalM * 100) : 0, exposedM: Math.round(exposedM), totalM: Math.round(totalM) };
    } else {
      score = computeExposureScore(coords);
    }
    displayExposureScore(score, avoidCams);

    document.getElementById('route-cams-count').textContent =
      avoidCams && data.cams_avoided > 0
        ? `${data.cams_avoided} caméra${data.cams_avoided > 1 ? 's' : ''} dans la zone`
        : '';

    const relaxedBanner = document.getElementById('route-relaxed-warn');
    if (relaxedBanner) relaxedBanner.style.display = data.relaxed ? 'block' : 'none';

    document.getElementById('direct-route-row').style.display = 'none';
    if (data.direct_route) {
      const coords2 = data.direct_route.route.coordinates.map(([lo, la]) => [la, lo]);
      drawRouteLayer(coords2, '#ffb300', '8,5', 0.6);
      document.getElementById('route-dist-direct').textContent = fmtDist(data.direct_route.distance_km);
      document.getElementById('route-time-direct').textContent = fmtTime(data.direct_route.duration_sec);
      document.getElementById('direct-route-row').style.display = 'flex';
    }

    document.getElementById('route-result').style.display    = 'block';
    document.getElementById('route-clear-btn').style.display = 'block';

    const trkpts = data.route.coordinates
      .map(([lo, la]) => `      <trkpt lat="${la.toFixed(6)}" lon="${lo.toFixed(6)}"></trkpt>`)
      .join('\n');
    const gpx = `<?xml version="1.0" encoding="UTF-8"?>\n<gpx version="1.1" creator="BlindSpot MTL" xmlns="http://www.topografix.com/GPX/1/1">\n  <trk>\n    <name>BlindSpot Route</name>\n    <trkseg>\n${trkpts}\n    </trkseg>\n  </trk>\n</gpx>`;
    document.getElementById('btn-gpx').href = URL.createObjectURL(new Blob([gpx], { type: 'application/gpx+xml' }));
    document.getElementById('nav-apps').style.display = 'block';

    saveToHistory({
      sLat: routeStart.lat, sLng: routeStart.lng,
      eLat: routeEnd.lat,   eLng: routeEnd.lng,
      preset: rangePreset, distKm: data.distance_km, durSec: data.duration_sec,
    });

    openMobilePanel();
    map.fitBounds(L.polyline(coords).getBounds(), { padding: [60, 60] });
    showToast(avoidCams
      ? (data.relaxed
          ? `⚠ Route partiellement sûre (zones réduites) : ${fmtDist(data.distance_km)}`
          : `✓ Route sûre : ${fmtDist(data.distance_km)} — ${data.cams_avoided} caméra(s) évitée(s)`)
      : `✓ Route : ${fmtDist(data.distance_km)}`);

  } catch (err) {
    showToast(`⚠ Erreur routing : ${err.message}`);
  } finally {
    btn.disabled = false; btn.textContent = '▶ CALCULER L\'ITINÉRAIRE';
  }
}

// ── URL sharing ──

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
    if (!isNaN(lat) && !isNaN(lng)) { setRoutePoint('end',   { lat, lng }); hasEnd   = true; }
  }
  if (params.p && ['conservative', 'standard', 'high'].includes(params.p)) setPreset(params.p);
  if (hasStart && hasEnd) setTimeout(calculateRoute, 600);
}

// ── Historique ──

const HISTORY_KEY = 'blindspot_history';
const HISTORY_MAX = 10;

function loadHistory()  { try { return JSON.parse(localStorage.getItem(HISTORY_KEY) || '[]'); } catch { return []; } }

function saveToHistory(entry) {
  const list = loadHistory();
  list.unshift({ ...entry, id: Date.now() });
  localStorage.setItem(HISTORY_KEY, JSON.stringify(list.slice(0, HISTORY_MAX)));
  renderHistory();
}

function deleteFromHistory(id) {
  localStorage.setItem(HISTORY_KEY, JSON.stringify(loadHistory().filter(e => e.id !== id)));
  renderHistory();
}

function timeAgo(ts) {
  const diff = Math.floor((Date.now() - ts) / 1000);
  if (diff < 60)    return 'à l\'instant';
  if (diff < 3600)  return `il y a ${Math.floor(diff/60)} min`;
  if (diff < 86400) return `il y a ${Math.floor(diff/3600)} h`;
  return `il y a ${Math.floor(diff/86400)} j`;
}

function renderHistory() {
  const list = loadHistory();
  const section   = document.getElementById('history-section');
  const container = document.getElementById('history-list');
  if (!list.length) { section.style.display = 'none'; return; }
  section.style.display = 'block';
  container.innerHTML = list.map(e => {
    const ago    = timeAgo(e.id);
    const dist   = e.distKm < 1 ? `${Math.round(e.distKm * 1000)} m` : `${e.distKm.toFixed(2)} km`;
    const preset = e.preset !== 'standard' ? ` · ${e.preset}` : '';
    return `<div class="history-item" onclick="restoreHistory(${e.id})">
      <div class="history-badges">
        <div class="history-badge a">A</div>
        <div class="history-badge b">B</div>
      </div>
      <div class="history-info">
        <div class="history-meta">${dist} · ~${Math.ceil(e.durSec/60)} min${preset}</div>
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

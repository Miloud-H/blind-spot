// ─── UI — Interactions, événements, boot ─────────────────────────────────────

// ── Toast ──

function showToast(msg) {
  const t = document.getElementById('toast');
  t.textContent = msg;
  t.classList.add('show');
  setTimeout(() => t.classList.remove('show'), 2800);
}

// ── Mobile panel ──

const sidebarEl = document.getElementById('sidebar');

function isMobile()        { return window.innerWidth <= 768; }
function openMobilePanel()  { if (isMobile()) sidebarEl.classList.add('panel-open');    }
function closeMobilePanel() { if (isMobile()) sidebarEl.classList.remove('panel-open'); }
function toggleMobilePanel(){ if (isMobile()) sidebarEl.classList.toggle('panel-open'); }

document.getElementById('sidebar-handle').addEventListener('click', toggleMobilePanel);

document.querySelector('.tabs').addEventListener('click', e => {
  if (isMobile() && !sidebarEl.classList.contains('panel-open')) {
    openMobilePanel(); e.stopPropagation();
  }
});

(function setupSwipe() {
  [document.getElementById('sidebar-handle'), document.querySelector('.tabs')].forEach(el => {
    let startY = 0;
    el.addEventListener('touchstart', e => { startY = e.touches[0].clientY; }, { passive: true });
    el.addEventListener('touchend',   e => {
      if (!isMobile()) return;
      const delta = startY - e.changedTouches[0].clientY;
      if (Math.abs(delta) < 20) return;
      if (delta > 0) openMobilePanel(); else closeMobilePanel();
    }, { passive: true });
  });
})();

// ── Onglets ──

function switchTab(name) {
  document.getElementById('tab-route').classList.toggle('active', name === 'route');
  document.getElementById('tab-cams').classList.toggle('active', name === 'cams');
  document.getElementById('content-route').style.display = name === 'route' ? 'flex' : 'none';
  document.getElementById('content-cams').style.display  = name === 'cams'  ? 'flex' : 'none';
  openMobilePanel();
}

// ── Presets ──

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
  const labels = { conservative: 'Évitement réduit — zones internes uniquement', standard: 'Évitement standard', high: 'Évitement maximal — zones étendues' };
  showToast(`◎ ${labels[name]}`);
  updateRouteHash();
}

// ── Ajout caméra ──

btnAdd.addEventListener('click', () => {
  addMode = true;
  routePickMode = null; clearPickButtons();
  map.getContainer().style.cursor = 'crosshair';
  modeBadge.textContent = '⊕ CLIQUER POUR PLACER'; modeBadge.classList.add('active');
  btnAdd.style.display = 'none'; btnCancel.style.display = 'block';
  showToast('Cliquer sur la carte pour placer la caméra');
});

btnCancel.addEventListener('click', () => {
  addMode = false; stopOrientTracking();
  map.getContainer().style.cursor = ''; modeBadge.classList.remove('active');
  btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
});

// ── Compas ──

const compass  = document.getElementById('compass');
const needle   = document.getElementById('compass-needle');
const dirInput = document.getElementById('cam-direction');

function setDirection(deg) {
  const v = ((Math.round(deg) % 360) + 360) % 360;
  dirInput.value = v;
  needle.style.transform = `translateX(-50%) translateY(-100%) rotate(${v}deg)`;
}

compass.addEventListener('click', e => {
  stopOrientTracking();
  const rect = compass.getBoundingClientRect();
  setDirection(toDeg(Math.atan2(e.clientX - (rect.left + rect.width/2), -(e.clientY - (rect.top + rect.height/2)))) + 360);
});

dirInput.addEventListener('input', () => { stopOrientTracking(); setDirection(parseFloat(dirInput.value) || 0); });

document.getElementById('cam-type').addEventListener('change', e => {
  document.getElementById('direction-group').style.display =
    (e.target.value === 'ptz' || e.target.value === 'unknown') ? 'none' : 'block';
});

// ── Orientation téléphone (compas) ──

let orientActive = false, orientGotHit = false, orientTimeout = null;

function _getHeading(e) {
  if (typeof e.webkitCompassHeading === 'number' && e.webkitCompassHeading >= 0) return e.webkitCompassHeading;
  if (e.absolute === true && typeof e.alpha === 'number' && e.alpha !== null) return (360 - e.alpha + 360) % 360;
  return null;
}

function _onOrient(e) {
  if (!orientActive) return;
  const h = _getHeading(e);
  if (h === null) return;
  if (!orientGotHit) { orientGotHit = true; clearTimeout(orientTimeout); showToast('◎ Compas actif — pointez vers la caméra'); }
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
  orientActive = false; clearTimeout(orientTimeout);
  window.removeEventListener('deviceorientationabsolute', _onOrient, true);
  window.removeEventListener('deviceorientation',         _onOrient, true);
  _setOrientBtnState(false);
}

async function startOrientTracking() {
  if (!window.DeviceOrientationEvent) { showToast('⚠ Boussole non disponible sur cet appareil'); return; }
  if (typeof DeviceOrientationEvent.requestPermission === 'function') {
    try {
      const p = await DeviceOrientationEvent.requestPermission();
      if (p !== 'granted') { showToast('⚠ Permission boussole refusée'); return; }
    } catch { showToast('⚠ Permission boussole refusée'); return; }
  }
  orientActive = true; orientGotHit = false;
  window.addEventListener('deviceorientationabsolute', _onOrient, true);
  window.addEventListener('deviceorientation',         _onOrient, true);
  _setOrientBtnState(true);
  orientTimeout = setTimeout(() => {
    if (orientActive && !orientGotHit) { showToast('⚠ Signal boussole absent — utilisez la roue'); stopOrientTracking(); }
  }, 2000);
}

document.getElementById('btn-orient').addEventListener('click', () => {
  if (orientActive) stopOrientTracking(); else startOrientTracking();
});

// ── Géolocalisation ──

let userLocCircle = null, userLocDot = null;
let geoWatchId = null, geoFollow = false;

function _updateGeoMarkers(lat, lng, accuracy) {
  if (userLocCircle) { map.removeLayer(userLocCircle); map.removeLayer(userLocDot); }
  userLocCircle = L.circle([lat, lng], {
    radius: accuracy, color: '#00b8d4', fillColor: '#00b8d4', fillOpacity: 0.1, weight: 1.5, dashArray: '4,4',
  }).addTo(map);
  userLocDot = L.circleMarker([lat, lng], {
    radius: 7, color: '#fff', fillColor: '#00b8d4', fillOpacity: 1, weight: 2,
  }).addTo(map);
  userLocCircle.bindPopup(`<div class="popup-title">📍 Votre position</div><div class="popup-row">Précision: <span>±${Math.round(accuracy)} m</span></div>`);
  userLocDot.on('click', () => userLocCircle.openPopup());
}

function _geoSetBtn(state) {
  const btn = document.getElementById('geoloc-btn');
  btn.classList.remove('locating', 'geo-follow', 'geo-nofollow');
  if (state === 'locating') btn.classList.add('locating');
  else if (state === 'follow')   btn.classList.add('geo-follow');
  else if (state === 'nofollow') btn.classList.add('geo-nofollow');
}

function _stopGeoWatch() {
  if (geoWatchId !== null) { navigator.geolocation.clearWatch(geoWatchId); geoWatchId = null; }
  geoFollow = false; _geoSetBtn('off');
  if (userLocCircle) { map.removeLayer(userLocCircle); userLocCircle = null; }
  if (userLocDot)    { map.removeLayer(userLocDot);    userLocDot    = null; }
}

function locateUser() {
  if (!navigator.geolocation) { showToast('⚠ Géolocalisation non supportée'); return; }
  if (geoWatchId === null) {
    _geoSetBtn('locating'); showToast('◎ Localisation en cours…');
    geoWatchId = navigator.geolocation.watchPosition(
      ({ coords: { latitude: lat, longitude: lng, accuracy } }) => {
        const first = !userLocDot;
        _updateGeoMarkers(lat, lng, accuracy);
        if (first) {
          _geoSetBtn('follow'); geoFollow = true;
          map.setView([lat, lng], 17, { animate: true });
          showToast(`✓ Suivi GPS actif (±${Math.round(accuracy)} m)`);
        } else if (geoFollow) {
          map.panTo([lat, lng], { animate: true, duration: 0.5 });
        }
      },
      err => {
        _stopGeoWatch();
        const msgs = { 1: 'Permission refusée', 2: 'Position indisponible', 3: 'Délai dépassé' };
        showToast(`⚠ Géoloc : ${msgs[err.code] || err.message}`);
      },
      { enableHighAccuracy: true, maximumAge: 5000, timeout: 15000 }
    );
  } else if (geoFollow) {
    _stopGeoWatch(); showToast('✕ Suivi GPS arrêté');
  } else {
    geoFollow = true; _geoSetBtn('follow');
    if (userLocDot) map.setView(userLocDot.getLatLng(), map.getZoom(), { animate: true });
  }
}

map.on('dragstart', () => {
  if (geoWatchId !== null && geoFollow) { geoFollow = false; _geoSetBtn('nofollow'); }
});

// ── Boutons routing ──

document.getElementById('btn-set-start').addEventListener('click', () => {
  addMode = false; btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
  if (routePickMode === 'start') {
    routePickMode = null; clearPickButtons(); map.getContainer().style.cursor = ''; modeBadge.classList.remove('active'); return;
  }
  routePickMode = 'start'; clearPickButtons();
  document.getElementById('btn-set-start').classList.add('active');
  map.getContainer().style.cursor = 'crosshair';
  modeBadge.textContent = 'A DÉFINIR LE DÉPART'; modeBadge.classList.add('active');
  closeMobilePanel(); showToast('Cliquer sur la carte pour définir le départ (A)');
});

document.getElementById('btn-set-end').addEventListener('click', () => {
  addMode = false; btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
  if (routePickMode === 'end') {
    routePickMode = null; clearPickButtons(); map.getContainer().style.cursor = ''; modeBadge.classList.remove('active'); return;
  }
  routePickMode = 'end'; clearPickButtons();
  document.getElementById('btn-set-end').classList.add('active');
  map.getContainer().style.cursor = 'crosshair';
  modeBadge.textContent = 'B DÉFINIR L\'ARRIVÉE'; modeBadge.classList.add('active');
  closeMobilePanel(); showToast('Cliquer sur la carte pour définir l\'arrivée (B)');
});

document.getElementById('route-btn').addEventListener('click', calculateRoute);
document.getElementById('route-clear-btn').addEventListener('click', clearRoute);

// ── Map click (handler unifié) ──

map.on('click', e => {
  if (routePickMode === 'start') {
    setRoutePoint('start', e.latlng);
  } else if (routePickMode === 'end') {
    setRoutePoint('end', e.latlng);
  } else if (addMode) {
    const typeVal  = document.getElementById('cam-type').value;
    const dirVal   = parseFloat(document.getElementById('cam-direction').value) || 0;
    const fovVal   = parseFloat(document.getElementById('cam-fov').value)       || 70;
    const rangeVal = parseFloat(document.getElementById('cam-range').value)     || 30;
    const noteVal  = document.getElementById('cam-note').value.trim();
    const cam = {
      lat: e.latlng.lat, lng: e.latlng.lng,
      direction: (typeVal === 'ptz' || typeVal === 'unknown') ? null : dirVal,
      fov: fovVal, range: rangeVal, type: typeVal,
      name: noteVal || 'Caméra communautaire', source: 'user', note: noteVal || null,
    };
    cameras.push(cam);
    mountCamera(cam, cameras.length - 1);
    persistCamera(cam);
    userCameraCount++; updateStats(); updateList();
    addMode = false; stopOrientTracking();
    map.getContainer().style.cursor = ''; modeBadge.classList.remove('active');
    btnAdd.style.display = 'block'; btnCancel.style.display = 'none';
    document.getElementById('cam-note').value = '';
    showToast('✓ Caméra ajoutée');
  }
});

// ── Viewport events ──

const syncViewportDebounced = debounce(syncViewport, 120);

map.on('moveend', () => {
  syncViewportDebounced();
  const bounds = map.getBounds().pad(0.4);
  const s = bounds.getSouth(), w = bounds.getWest(), n = bounds.getNorth(), e = bounds.getEast();
  if (viewportNeedsLoad(bounds)) loadCamerasForBbox(s, w, n, e);
  else loadBuildingsForBbox(s, w, n, e);
});

map.on('zoomend', syncViewportDebounced);

// ── Admin: jump to camera (?lat=&lng=&z=&cam=) ──

function restoreFromQuery() {
  const p   = new URLSearchParams(location.search);
  const lat = parseFloat(p.get('lat'));
  const lng = parseFloat(p.get('lng'));
  const z   = parseInt(p.get('z')) || 18;
  const cam = parseInt(p.get('cam'));
  if (isNaN(lat) || isNaN(lng)) return;
  map.setView([lat, lng], z);
  const ring = L.circleMarker([lat, lng], { radius: 20, color: '#00ff41', fill: false, weight: 2, opacity: 1 }).addTo(map);
  let op = 1;
  const fade = setInterval(() => {
    op -= 0.04; ring.setStyle({ opacity: Math.max(0, op) });
    if (op <= 0) { clearInterval(fade); map.removeLayer(ring); }
  }, 80);
  if (!isNaN(cam)) _highlightCamId = cam;
}

// ── Sidebar resize (desktop) ──

(function () {
  const sidebar = document.getElementById('sidebar');
  const handle  = document.getElementById('sidebar-resize');
  const KEY     = 'blindspot_sidebar_w';
  const saved   = parseInt(localStorage.getItem(KEY));
  if (saved && window.innerWidth > 768) sidebar.style.width = saved + 'px';
  handle.addEventListener('mousedown', e => {
    if (window.innerWidth <= 768) return;
    e.preventDefault(); handle.classList.add('dragging');
    const startX = e.clientX, startW = sidebar.getBoundingClientRect().width;
    function onMove(e) { sidebar.style.width = Math.min(520, Math.max(220, startW + (startX - e.clientX))) + 'px'; }
    function onUp()   { handle.classList.remove('dragging'); localStorage.setItem(KEY, parseInt(sidebar.style.width)); document.removeEventListener('mousemove', onMove); document.removeEventListener('mouseup', onUp); }
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup',   onUp);
  });
})();

// ── Boot ──

setTimeout(() => map.invalidateSize(), 100);
document.getElementById('geoloc-btn').addEventListener('click', locateUser);
restoreFromQuery();
loadCameras();
renderHistory();
restoreFromHash();

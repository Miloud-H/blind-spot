// ─── HEATMAP — Densité caméras (zones froides = à explorer) ─────────────────

let _heatLayer  = null;
let _heatEnabled = false;

function toggleHeatmap() {
  if (_heatEnabled) {
    _removeHeatmap();
  } else {
    _showHeatmap();
  }
}

function _showHeatmap() {
  if (!window.L || !L.heatLayer) return;

  const pts = (typeof cameras !== 'undefined' ? cameras : [])
    .map(c => [c.lat, c.lng, 1]);

  if (pts.length === 0) return;

  // Masquer marqueurs et zones pour ne pas parasiter la lecture
  if (typeof renderedCameras !== 'undefined' && typeof unmountCamera === 'function') {
    for (const k of [...renderedCameras.keys()]) unmountCamera(k);
  }

  _heatLayer = L.heatLayer(pts, {
    radius:  45,
    blur:    30,
    maxZoom: 17,
    max:     3,   // 3 caméras en overlap = saturation rouge
    gradient: { 0.05: '#1a237e', 0.35: '#ff6f00', 1.0: '#ff1744' },
  }).addTo(map);

  _heatEnabled = true;
  document.getElementById('btn-heatmap').classList.add('active');

  map.on('moveend', _refreshHeatmap);
}

function _refreshHeatmap() {
  if (!_heatEnabled || !_heatLayer) return;
  const pts = (typeof cameras !== 'undefined' ? cameras : [])
    .map(c => [c.lat, c.lng, 1]);
  _heatLayer.setLatLngs(pts);
}

function _removeHeatmap() {
  if (_heatLayer) { map.removeLayer(_heatLayer); _heatLayer = null; }
  map.off('moveend', _refreshHeatmap);
  _heatEnabled = false;
  document.getElementById('btn-heatmap').classList.remove('active');

  // Restaurer les marqueurs et zones
  if (typeof syncViewport === 'function') syncViewport();
}

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

  // cameras est défini dans cameras.js
  const pts = (typeof cameras !== 'undefined' ? cameras : [])
    .map(c => [c.lat, c.lng, 1]);

  if (pts.length === 0) return;

  _heatLayer = L.heatLayer(pts, {
    radius:  35,
    blur:    25,
    maxZoom: 17,
    max:     8,
    gradient: { 0.0: '#00000000', 0.2: '#0d47a1', 0.5: '#ff6f00', 1.0: '#ff1744' },
  }).addTo(map);

  _heatEnabled = true;
  document.getElementById('btn-heatmap').classList.add('active');

  // Rafraîchir si de nouvelles caméras se chargent
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
}

// ── Données de démo ──────────────────────────────────────────────────────────
// Utilisé uniquement en fallback quand le backend est inaccessible (voir cameras.js),
// pour que l'interface reste explorable même hors-ligne / en développement isolé.

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

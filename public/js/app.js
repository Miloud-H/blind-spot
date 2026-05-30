// ─── MAP ─────────────────────────────────────────────────────────────────────

const map = L.map('map', { center: [45.5231, -73.5982], zoom: 15, preferCanvas: true });

L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', {
  attribution: '© OpenStreetMap contributors', maxZoom: 19,
}).addTo(map);

// ─── GLOBALS PARTAGÉS ────────────────────────────────────────────────────────
// Déclarés ici car utilisés à la fois dans routing.js et ui.js

let addMode = false;

const modeBadge = document.getElementById('mode-badge');
const btnAdd    = document.getElementById('btn-add-mode');
const btnCancel = document.getElementById('btn-cancel');

const STATIC_CACHE = 'bs-static-v2'; // bump → vide l'ancien cache
const TILE_CACHE   = 'bs-tiles-v1';
const API_CACHE    = 'bs-api-v1';

const TILE_MAX = 400;

// Seuls les ressources stables sont pré-cachées — JS/CSS ont des ?v= qui changent à chaque build
const PRECACHE = ['/', '/manifest.json', '/icons/icon.svg'];

// ── Install: pre-cache app shell ──────────────────────────────
self.addEventListener('install', e => {
  e.waitUntil(
    caches.open(STATIC_CACHE)
      .then(c => Promise.allSettled(PRECACHE.map(url => c.add(url))))
      .then(() => self.skipWaiting())
  );
});

// ── Activate: remove old caches ───────────────────────────────
self.addEventListener('activate', e => {
  const keep = new Set([STATIC_CACHE, TILE_CACHE, API_CACHE]);
  e.waitUntil(
    caches.keys()
      .then(keys => Promise.all(keys.filter(k => !keep.has(k)).map(k => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

// ── Fetch ─────────────────────────────────────────────────────
self.addEventListener('fetch', e => {
  if (e.request.method !== 'GET') return;

  const url = new URL(e.request.url);

  // OSM tiles → cache first, capped at TILE_MAX entries (FIFO eviction)
  if (url.hostname.endsWith('tile.openstreetmap.org')) {
    e.respondWith(tileFirst(e.request));
    return;
  }

  // CDN (Leaflet, Google Fonts) → stale-while-revalidate
  if (url.hostname.includes('cdnjs.cloudflare.com') ||
      url.hostname.includes('fonts.googleapis.com') ||
      url.hostname.includes('fonts.gstatic.com')) {
    e.respondWith(staleWhileRevalidate(e.request, STATIC_CACHE));
    return;
  }

  // /api/cameras → network first, cache fallback (serves stale when offline)
  if (url.pathname.startsWith('/api/cameras')) {
    e.respondWith(networkFirst(e.request, API_CACHE));
    return;
  }

  // /api/route → never cache (too dynamic + expensive)
  if (url.pathname.startsWith('/api/')) return;

  // App shell (/, .css, .js, icons, manifest) → network-first
  // Toujours la version fraîche si en ligne ; cache en fallback hors ligne.
  if (url.origin === self.location.origin) {
    e.respondWith(networkFirst(e.request, STATIC_CACHE));
  }
});

// ── Strategies ────────────────────────────────────────────────
async function tileFirst(req) {
  const cache = await caches.open(TILE_CACHE);
  const cached = await cache.match(req);
  if (cached) return cached;

  try {
    const res = await fetch(req);
    if (res.ok) {
      const keys = await cache.keys();
      if (keys.length >= TILE_MAX) {
        // FIFO: evict oldest 20 at once to amortise the cost
        await Promise.all(keys.slice(0, 20).map(k => cache.delete(k)));
      }
      cache.put(req, res.clone());
    }
    return res;
  } catch {
    return new Response(null, { status: 503, statusText: 'Tile unavailable offline' });
  }
}

async function staleWhileRevalidate(req, cacheName) {
  const cache  = await caches.open(cacheName);
  const cached = await cache.match(req);
  const fresh  = fetch(req).then(res => {
    if (res.ok) cache.put(req, res.clone());
    return res;
  }).catch(() => null);
  return cached ?? fresh;
}

async function networkFirst(req, cacheName) {
  const cache = await caches.open(cacheName);
  try {
    const res = await fetch(req);
    if (res.ok) cache.put(req, res.clone());
    return res;
  } catch {
    const cached = await cache.match(req);
    return cached ?? new Response(
      JSON.stringify({ error: 'Hors ligne — données en cache indisponibles' }),
      { status: 503, headers: { 'Content-Type': 'application/json' } }
    );
  }
}

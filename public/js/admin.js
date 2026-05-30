// ── API ──────────────────────────────────────────────────────────────────────
const api = (path, opts = {}) => fetch(path, {
  ...opts,
  headers: {
    'Authorization': `Bearer ${document.getElementById('token').value.trim()}`,
    'Content-Type': 'application/json',
    ...(opts.headers || {}),
  },
});

// ── Toast ─────────────────────────────────────────────────────────────────────
function toast(msg, err = false) {
  const el = document.getElementById('toast');
  el.textContent = msg;
  el.className = 'toast' + (err ? ' err' : '');
  el.style.display = 'block';
  clearTimeout(el._t);
  el._t = setTimeout(() => el.style.display = 'none', 3200);
}

// ── Log ───────────────────────────────────────────────────────────────────────
function logLine(msg, cls = '') {
  const el = document.getElementById('act-log');
  const d = document.createElement('div');
  d.className = cls;
  d.textContent = `▶ ${msg}`;
  el.appendChild(d);
  el.scrollTop = el.scrollHeight;
}

// ── Navigation ────────────────────────────────────────────────────────────────
function goto(id) {
  document.querySelectorAll('.section').forEach(s => s.classList.remove('active'));
  document.querySelectorAll('.nav-item').forEach(n => n.classList.remove('active'));
  document.getElementById('section-' + id).classList.add('active');
  document.getElementById('nav-' + id).classList.add('active');
  if (id === 'cameras') loadCameras(1);
  if (id === 'reports') loadReports();
  if (id === 'export')  loadExportStats();
  if (id === 'zones')   setTimeout(initZonesSection, 30);
}

// ── Load all ──────────────────────────────────────────────────────────────────
async function loadAll() {
  const res = await api('/api/admin/stats');
  const st = document.getElementById('tokst');
  if (!res.ok) {
    st.textContent = '✕ TOKEN INVALIDE'; st.className = 'tokst err';
    toast('Token invalide', true); return;
  }
  st.textContent = '✓ AUTHENTIFIÉ'; st.className = 'tokst ok';
  const d = await res.json();
  renderDash(d);

  const active = document.querySelector('.section.active')?.id.replace('section-','');
  if (active === 'cameras') loadCameras(1);
  if (active === 'reports') loadReports();
  if (active === 'export')  loadExportStats();
}

// ── Dashboard ─────────────────────────────────────────────────────────────────
function renderDash(d) {
  document.getElementById('dc-total').textContent = d.cameras_total ?? '—';
  document.getElementById('dc-osm').textContent   = d.cameras_osm ?? '—';
  document.getElementById('dc-user').textContent  = d.cameras_user ?? '—';
  document.getElementById('dc-inf').textContent   = d.cameras_inferred ?? '—';
  document.getElementById('dc-rep').textContent   = d.cameras_reported ?? '—';
  document.getElementById('dc-cache').textContent = d.route_cache_size ?? '—';
  document.getElementById('cache-sz').textContent = d.route_cache_size ?? '—';

  const t = d.cameras_total || 1;
  document.getElementById('chart-src').innerHTML =
    bar('OSM',        d.cameras_osm      ?? 0, t, '') +
    bar('COMMUNAUTÉ', d.cameras_user     ?? 0, t, 'amber') +
    bar('INFÉRÉES',   d.cameras_inferred ?? 0, t, 'cyan');

  document.getElementById('chart-typ').innerHTML =
    bar('FIXE',     d.type_fixed   ?? 0, t, '') +
    bar('PTZ/DÔME', d.type_ptz     ?? 0, t, 'amber') +
    bar('INCONNUE', d.type_unknown ?? 0, t, 'cyan');

  const badge = document.getElementById('nbadge-rep');
  const nr = d.cameras_reported ?? 0;
  badge.textContent = nr;
  badge.classList.toggle('on', nr > 0);
}

function bar(label, v, total, cls) {
  const pct = total > 0 ? Math.round(v / total * 100) : 0;
  return `<div class="br">
    <div class="bl">${label}</div>
    <div class="bt"><div class="bf ${cls}" style="width:${pct}%"></div></div>
    <div class="bv">${v} <span style="color:var(--dim);font-size:9px;">(${pct}%)</span></div>
  </div>`;
}

// ── Edit modal ────────────────────────────────────────────────────────────────
const _camMap = {};
let _editId   = null;
let _editMap  = null;
let _editMark = null;

const _camIcon = () => L.divIcon({
  html: '<div style="width:14px;height:14px;background:#00ff41;border:2px solid #050a06;border-radius:50%;box-shadow:0 0 6px #00ff41;cursor:grab"></div>',
  iconSize: [14, 14], iconAnchor: [7, 7], className: '',
});

function _initEditMap(lat, lng) {
  if (_editMap) {
    _editMap.setView([lat, lng], 18);
    _editMark.setLatLng([lat, lng]);
    _editMap.invalidateSize();
    return;
  }
  _editMap = L.map('ed-map', { zoomControl: true, attributionControl: false })
              .setView([lat, lng], 18);
  L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', { maxZoom: 21 })
   .addTo(_editMap);

  _editMark = L.marker([lat, lng], { draggable: true, icon: _camIcon() })
               .addTo(_editMap);

  _editMark.on('dragend', () => {
    const p = _editMark.getLatLng();
    document.getElementById('ed-lat').value = p.lat.toFixed(6);
    document.getElementById('ed-lng').value = p.lng.toFixed(6);
  });

  _editMap.on('click', e => {
    _editMark.setLatLng(e.latlng);
    document.getElementById('ed-lat').value = e.latlng.lat.toFixed(6);
    document.getElementById('ed-lng').value = e.latlng.lng.toFixed(6);
  });
}

function syncMarkerFromFields() {
  if (!_editMark) return;
  const lat = parseFloat(document.getElementById('ed-lat').value);
  const lng = parseFloat(document.getElementById('ed-lng').value);
  if (!isNaN(lat) && !isNaN(lng)) {
    _editMark.setLatLng([lat, lng]);
    _editMap.panTo([lat, lng]);
  }
}

function openEdit(id) {
  const cam = _camMap[id];
  if (!cam) return;
  _editId = id;
  document.getElementById('edit-cam-id').textContent = `#${id}`;
  document.getElementById('ed-lat').value   = cam.lat;
  document.getElementById('ed-lng').value   = cam.lng;
  document.getElementById('ed-dir').value   = cam.direction ?? '';
  document.getElementById('ed-fov').value   = cam.fov ?? 70;
  document.getElementById('ed-range').value = cam.range_m ?? 30;
  document.getElementById('ed-type').value  = cam.cam_type ?? 'unknown';
  document.getElementById('ed-name').value  = cam.name ?? '';
  document.getElementById('ed-note').value  = cam.note ?? '';
  document.getElementById('edit-modal').style.display = 'flex';
  setTimeout(() => _initEditMap(cam.lat, cam.lng), 30);
}

function closeEdit(e) {
  if (e && e.target !== document.getElementById('edit-modal')) return;
  document.getElementById('edit-modal').style.display = 'none';
  _editId = null;
}

async function submitEdit() {
  if (!_editId) return;
  const dirStr  = document.getElementById('ed-dir').value.trim();
  const nameStr = document.getElementById('ed-name').value.trim();
  const noteStr = document.getElementById('ed-note').value.trim();
  const body = {
    lat:       parseFloat(document.getElementById('ed-lat').value),
    lng:       parseFloat(document.getElementById('ed-lng').value),
    direction: dirStr !== '' ? parseFloat(dirStr) : null,
    fov:       parseFloat(document.getElementById('ed-fov').value),
    range_m:   parseFloat(document.getElementById('ed-range').value),
    cam_type:  document.getElementById('ed-type').value,
    name:      nameStr || null,
    note:      noteStr || null,
  };
  if (isNaN(body.lat) || isNaN(body.lng)) { toast('⚠ Coordonnées invalides', true); return; }
  const res = await api(`/api/admin/cameras/${_editId}`, {
    method: 'PATCH', body: JSON.stringify(body),
  });
  if (res.ok) {
    toast(`✓ Caméra ${_editId} mise à jour`);
    document.getElementById('edit-modal').style.display = 'none';
    _editId = null;
    loadCameras(_camPage);
  } else {
    const d = await res.json().catch(() => ({}));
    toast(`⚠ ${d.error || 'Erreur mise à jour'}`, true);
  }
}

// ── Camera list ───────────────────────────────────────────────────────────────
let _camPage = 1;
async function loadCameras(page = 1) {
  _camPage = page;
  const src = document.getElementById('f-src').value;
  const typ = document.getElementById('f-type').value;
  const rep = document.getElementById('f-rep').checked;
  const p = new URLSearchParams({ page, limit: 50 });
  if (src) p.set('source', src);
  if (typ) p.set('cam_type', typ);
  if (rep) p.set('reported', 'true');

  const res = await api(`/api/admin/cameras?${p}`);
  if (!res.ok) { toast('Erreur chargement', true); return; }
  const d = await res.json();

  document.getElementById('cams-sub').textContent      = `${d.total} caméra(s)`;
  document.getElementById('cams-pageinfo').textContent = `Page ${d.page} / ${Math.max(1, d.pages)}`;

  const tb = document.getElementById('cams-tbody');
  if (!d.cameras.length) {
    tb.innerHTML = `<tr><td colspan="8"><div class="empty">AUCUNE CAMÉRA</div></td></tr>`;
  } else {
    d.cameras.forEach(c => { _camMap[c.id] = c; });
    tb.innerHTML = d.cameras.map(c => `
      <tr id="cr-${c.id}">
        <td class="td-m">${c.id}</td>
        <td class="td-d td-m">${c.lat.toFixed(5)}, ${c.lng.toFixed(5)}</td>
        <td><span class="bg ${c.cam_type}">${c.cam_type.toUpperCase()}</span></td>
        <td><span class="bg ${c.source}">${c.source.toUpperCase()}</span></td>
        <td class="td-d">${c.direction != null ? c.direction.toFixed(0)+'°' : '—'}</td>
        <td style="${c.report_count > 0 ? 'color:var(--red);font-weight:bold' : 'color:var(--dim)'}">
          ${c.report_count > 0 ? '⚠ ' : ''}${c.report_count}
        </td>
        <td>
          <div style="display:flex;gap:4px;">
            <a class="maplnk" href="/?lat=${c.lat}&lng=${c.lng}&z=18&cam=${c.id}" target="_blank">🗺</a>
            ${c.source !== 'osm' ? `<button class="sm amber" onclick="openEdit(${c.id})" title="Modifier">✎</button>` : ''}
            <button class="sm danger" onclick="deleteCamera(${c.id},'cr-${c.id}')">✕</button>
          </div>
        </td>
      </tr>
    `).join('');
  }

  const pag = document.getElementById('cams-pag');
  const pages = Math.max(1, d.pages);
  if (pages <= 1) { pag.innerHTML = ''; return; }
  let h = `<button ${page <= 1 ? 'disabled' : ''} onclick="loadCameras(${page-1})">◀</button>`;
  const s = Math.max(1, page-2), e = Math.min(pages, page+2);
  for (let p2 = s; p2 <= e; p2++) {
    const act = p2 === page ? 'style="border-color:var(--green);background:rgba(0,255,65,0.1)"' : '';
    h += `<button ${act} onclick="loadCameras(${p2})">${p2}</button>`;
  }
  h += `<button ${page >= pages ? 'disabled' : ''} onclick="loadCameras(${page+1})">▶</button>`;
  h += `<span class="pagi">— ${d.total} entrées</span>`;
  pag.innerHTML = h;
}

function resetFilters() {
  document.getElementById('f-src').value = '';
  document.getElementById('f-type').value = '';
  document.getElementById('f-rep').checked = false;
  loadCameras(1);
}

// ── Signalements ──────────────────────────────────────────────────────────────
async function loadReports() {
  const res = await api('/api/admin/reports');
  if (!res.ok) { toast('Erreur', true); return; }
  const cams = await res.json();

  const badge = document.getElementById('nbadge-rep');
  badge.textContent = cams.length;
  badge.classList.toggle('on', cams.length > 0);

  const tb = document.getElementById('rep-tbody');
  if (!cams.length) {
    tb.innerHTML = `<tr><td colspan="7"><div class="empty">✓ AUCUN SIGNALEMENT</div></td></tr>`;
    return;
  }
  tb.innerHTML = cams.map(c => `
    <tr id="rr-${c.id}">
      <td class="td-m">${c.id}</td>
      <td class="td-d td-m">${c.lat.toFixed(5)}, ${c.lng.toFixed(5)}</td>
      <td><span class="bg ${c.cam_type}">${c.cam_type.toUpperCase()}</span></td>
      <td><span class="bg ${c.source}">${c.source.toUpperCase()}</span></td>
      <td class="td-d" style="max-width:160px;overflow:hidden;text-overflow:ellipsis;">${c.name || '—'}</td>
      <td style="color:var(--red);font-weight:bold;">⚠ ${c.report_count}</td>
      <td>
        <div style="display:flex;gap:4px;">
          <a class="maplnk" href="/?lat=${c.lat}&lng=${c.lng}&z=18&cam=${c.id}" target="_blank">🗺</a>
          <button class="sm danger" onclick="deleteCamera(${c.id},'rr-${c.id}',true)">✕</button>
        </div>
      </td>
    </tr>
  `).join('');
}

async function deleteAllReported() {
  if (!confirm('Supprimer toutes les caméras signalées ?')) return;
  const res = await api('/api/admin/reports');
  if (!res.ok) return;
  const cams = await res.json();
  if (!cams.length) { toast('Aucune caméra signalée'); return; }

  const r = await api('/api/admin/cameras', {
    method: 'DELETE',
    body: JSON.stringify({ ids: cams.map(c => c.id) }),
  });
  if (r.ok) {
    const d = await r.json();
    toast(`✓ ${d.deleted} caméra(s) supprimée(s)`);
    loadReports();
    loadAll();
  } else {
    toast('⚠ Erreur suppression', true);
  }
}

// ── Delete ────────────────────────────────────────────────────────────────────
async function deleteCamera(id, rowId, reloadRep = false) {
  const res = await api(`/api/admin/cameras/${id}`, { method: 'DELETE' });
  if (res.ok) {
    document.getElementById(rowId)?.remove();
    toast(`✓ Caméra ${id} supprimée`);
    loadAll();
    if (reloadRep) loadReports();
  } else {
    toast('⚠ Erreur suppression', true);
  }
}

// ── Export OSM ────────────────────────────────────────────────────────────────
async function loadExportStats() {
  const res = await api('/api/admin/stats');
  if (!res.ok) return;
  const d = await res.json();
  const userTotal    = d.cameras_user ?? 0;
  const userReported = d.cameras_user_reported ?? 0;
  document.getElementById('ex-count').textContent = userTotal - userReported;
  document.getElementById('ex-dir').textContent   = d.cameras_with_direction ?? '—';
  document.getElementById('ex-excl').textContent  = userReported;
}

async function downloadExport() {
  const res = await api('/api/admin/export/osm');
  if (!res.ok) { toast('Erreur export ou non autorisé', true); return; }
  const blob = await res.blob();
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url; a.download = 'blindspot-export.osm';
  document.body.appendChild(a); a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
  toast('✓ Export téléchargé');
}

// ── Actions ───────────────────────────────────────────────────────────────────
async function doReseed() {
  if (!confirm('Re-importer toutes les caméras depuis OSM ? (peut prendre 30 s)')) return;
  logLine('Import OSM en cours…');
  const res = await api('/api/admin/reseed', { method: 'POST' });
  const d = await res.json();
  if (res.ok) { logLine(`✓ ${d.message}`, 'ok'); loadAll(); }
  else logLine(`⚠ Erreur: ${d.error || res.status}`, 'err');
}

async function doClearCache() {
  const res = await api('/api/admin/cache', { method: 'DELETE' });
  if (res.ok) { logLine('✓ Cache routes vidé', 'ok'); loadAll(); }
  else logLine('⚠ Erreur vider cache', 'err');
}

// ── Zones d'évitement ─────────────────────────────────────────────────────────
let _zonesMap    = null;
let _zonesLayers = [];

function initZonesSection() {
  if (_zonesMap) { _zonesMap.invalidateSize(); return; }
  _zonesMap = L.map('z-map', { attributionControl: false })
               .setView([45.5231, -73.5982], 15);
  L.tileLayer('https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png', { maxZoom: 19 })
   .addTo(_zonesMap);
  L.control.attribution({ prefix: '© OSM' }).addTo(_zonesMap);
}

async function loadZones() {
  if (!_zonesMap) initZonesSection();
  _zonesMap.invalidateSize();

  const b = _zonesMap.getBounds();
  const bbox = [
    b.getSouth().toFixed(5), b.getWest().toFixed(5),
    b.getNorth().toFixed(5), b.getEast().toFixed(5),
  ].join(',');
  const preset = document.getElementById('z-preset').value;

  _zonesLayers.forEach(l => _zonesMap.removeLayer(l));
  _zonesLayers = [];
  document.getElementById('z-stats').textContent = '⟳ Chargement…';

  const res = await api(`/api/admin/zones?bbox=${bbox}&preset=${preset}`);
  if (!res.ok) {
    const d = await res.json().catch(() => ({}));
    toast(`⚠ ${d.error || 'Erreur zones'}`, true);
    document.getElementById('z-stats').textContent = '';
    return;
  }
  const data = await res.json();

  document.getElementById('z-stats').textContent =
    `${data.cameras_count} caméras → ${data.raw_count} brutes → ${data.merged_count} fusionnées`;

  data.features.forEach(f => {
    const coords = f.geometry.coordinates[0].map(([lng, lat]) => [lat, lng]);
    const poly = L.polygon(coords, {
      color: '#ff3131', fillColor: '#ff3131', weight: 1.2,
      fillOpacity: 0.22, opacity: 0.85,
    }).addTo(_zonesMap);
    _zonesLayers.push(poly);
  });

  if (data.merged_count > 0) {
    toast(`✓ ${data.merged_count} zone(s) affichée(s)`);
  } else {
    toast('⚠ Aucune zone dans cette vue');
  }
}

// ── Enter on token ────────────────────────────────────────────────────────────
document.getElementById('token').addEventListener('keydown', e => {
  if (e.key === 'Enter') loadAll();
});

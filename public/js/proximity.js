// ─── PROXIMITY — Alerte vibration à l'approche d'une caméra ─────────────────
//
// Utilise watchPosition pour tracker la position GPS.
// Vibre quand l'utilisateur entre dans une zone de caméra.

let _proximityEnabled = false;
let _watchId          = null;
let _wasInZone        = false;

const VIBRATE_ENTER = [100, 60, 100]; // deux impulsions courtes = entrée en zone
const VIBRATE_EXIT  = [30];           // impulsion très courte = sortie de zone

function toggleProximityAlert() {
  if (_proximityEnabled) {
    _stopProximity();
  } else {
    _startProximity();
  }
}

function _startProximity() {
  if (!('geolocation' in navigator)) {
    console.warn('Géolocalisation non disponible');
    return;
  }
  if (!('vibrate' in navigator)) {
    console.warn('Vibration non disponible sur cet appareil');
  }

  _proximityEnabled = true;
  _wasInZone = false;
  document.getElementById('btn-proximity').classList.add('active');

  _watchId = navigator.geolocation.watchPosition(
    _onPosition,
    err => console.warn('Proximité GPS:', err.message),
    { enableHighAccuracy: true, maximumAge: 2000, timeout: 10000 }
  );
}

function _stopProximity() {
  _proximityEnabled = false;
  if (_watchId !== null) {
    navigator.geolocation.clearWatch(_watchId);
    _watchId = null;
  }
  _wasInZone = false;
  document.getElementById('btn-proximity').classList.remove('active');
}

function _onPosition(pos) {
  const { latitude: lat, longitude: lng } = pos.coords;

  // cameras est défini dans cameras.js — déjà chargé pour la vue courante
  const inZone = (typeof cameras !== 'undefined') &&
    cameras.some(cam => isPointInCameraZone(lat, lng, cam));

  if (inZone && !_wasInZone) {
    navigator.vibrate(VIBRATE_ENTER);
  } else if (!inZone && _wasInZone) {
    navigator.vibrate(VIBRATE_EXIT);
  }

  _wasInZone = inZone;
}

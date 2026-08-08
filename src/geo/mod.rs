//! Calcul géométrique des cônes de surveillance — découpé par responsabilité.
//! Reproduit la logique JS du prototype côté serveur.
//!
//! - `math`     : primitives pures (Haversine, cap, point de destination)
//! - `exposure` : test point/segment dans une zone de caméra
//! - `shapes`   : génération cône/cercle (fallback sans données bâtiment)
//! - `viewshed` : ray-casting LOS contre les bâtiments + génération des rings ORS
//! - `polygons` : fusion de rings, marge de sécurité, point-in-polygon

// Certaines fonctions ré-exportées ici ne sont pas (encore) appelées hors du module `geo`
// (utilisées seulement en interne entre sous-modules, ou dans leurs propres tests) mais
// faisaient déjà partie de l'API publique de l'ancien geo.rs monolithique — on préserve
// cette surface plutôt que de la restreindre silencieusement pendant le découpage.
#![allow(unused_imports)]

mod exposure;
mod math;
mod polygons;
mod shapes;
mod viewshed;

pub use exposure::{compute_segment_exposure, point_in_camera_zone, segment_in_camera_zone};
pub use math::{dist_to_segment_approx, haversine_m};
pub use polygons::{add_ors_safety_margin, filter_rings_containing_endpoints, merge_overlapping_rings, point_in_polygon};
pub use shapes::{build_circle, build_cone};
pub use viewshed::{cameras_to_ors_rings, compute_viewshed};

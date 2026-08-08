//! Accès base de données, découpé par domaine.
//! Chaque sous-module regroupe les requêtes d'une même table/responsabilité ;
//! tout est ré-exporté ici pour que les appelants continuent d'écrire
//! `db::get_cameras_in_bbox(...)`, `db::insert_camera(...)`, etc. sans changement.

mod buildings;
mod cameras;
mod metadata;
mod routing;

pub use buildings::*;
pub use cameras::*;
pub use metadata::*;
pub use routing::*;

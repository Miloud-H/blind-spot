//! Orchestration des tâches de seed lancées au démarrage du serveur
//! (caméras OSM/inférées, bâtiments, graphe routier A*).
//! Tout tourne en arrière-plan — le serveur HTTP démarre immédiatement.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sqlx::SqlitePool;
use crate::{db, services};

/// Lance les tâches de seed en arrière-plan et retourne le flag partagé `routing_ready`,
/// mis à `true` une fois le graphe routier A* complètement construit.
/// `event_bus` : diffuse les suppressions de caméras obsolètes (purge post-reseed) en temps réel.
pub async fn spawn_seed_tasks(
    pool: &SqlitePool,
    http_client: &reqwest::Client,
    event_bus: &tokio::sync::broadcast::Sender<String>,
) -> Arc<AtomicBool> {
    // ── Auto-seed / re-seed Overpass ─────────────────────────────────────────
    // Conditions de (re-)seed :
    //   • Base OSM vide (premier lancement)
    //   • Dernier import OSM > 7 jours
    let cam_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'osm'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);

    let days_old = db::days_since_osm_seed(pool).await;
    let should_seed = cam_count == 0 || days_old >= 7;

    let bld_count: i64   = db::count_buildings(pool).await;
    let edge_count: i64  = db::count_routing_edges(pool).await;
    // Le graphe est "prêt" si le seed précédent s'est terminé avec succès
    let graph_ready_persisted = db::get_metadata(pool, "routing_graph_ready")
        .await.ok().flatten().map(|v| v == "1").unwrap_or(false);
    let routing_ready = Arc::new(AtomicBool::new(graph_ready_persisted && edge_count > 0));
    if graph_ready_persisted && edge_count > 0 {
        tracing::info!("Routeur A* disponible ({edge_count} arêtes)");
    }

    // ── Seed caméras + bâtiments (conditionnel : base vide ou données > 7 jours) ─
    if should_seed {
        if cam_count == 0 {
            tracing::info!("Base OSM vide — import initial depuis Overpass en arrière-plan…");
        } else {
            tracing::info!(
                "Données OSM âgées de {} jour(s) — re-seed en arrière-plan…",
                days_old
            );
        }
        let pool_bg    = pool.clone();
        let client_bg  = http_client.clone();
        let ready_flag = routing_ready.clone();
        let events_bg  = event_bus.clone();
        tokio::spawn(async move {
            match services::overpass::seed_from_overpass(&pool_bg, &client_bg, &events_bg).await {
                Ok(n)  => tracing::info!("Seed OSM terminé : {n} caméras importées/mises à jour"),
                Err(e) => tracing::warn!("Seed OSM échoué : {e}"),
            }
            match services::inferred::seed_inferred_cameras(&pool_bg, &client_bg, &events_bg).await {
                Ok(n)  => tracing::info!("Seed inféré terminé : {n} caméras déduites importées"),
                Err(e) => tracing::warn!("Seed inféré échoué : {e}"),
            }
            if bld_count == 0 {
                match services::buildings::seed_buildings(&pool_bg, &client_bg).await {
                    Ok(n)  => tracing::info!("Seed bâtiments terminé : {n} bâtiments insérés"),
                    Err(e) => tracing::warn!("Seed bâtiments échoué : {e}"),
                }
            } else {
                tracing::info!("{bld_count} bâtiments déjà en base — seed ignoré");
            }
            // Recalcul complet des expositions si le graphe A* est déjà seedé
            if ready_flag.load(Ordering::Relaxed) {
                tracing::info!("Re-seed caméras terminé — recalcul des expositions du graphe A*...");
                if let Err(e) = db::reset_edge_exposures(&pool_bg).await {
                    tracing::warn!("Reset expositions échoué : {e}");
                } else {
                    match services::routing_graph::compute_edge_exposures(&pool_bg).await {
                        Ok(u)  => tracing::info!("Expositions recalculées : {u} arêtes exposées"),
                        Err(e) => tracing::warn!("Recalcul exposition échoué : {e}"),
                    }
                }
            }
        });
    } else {
        tracing::info!(
            "{cam_count} caméras OSM en base (import il y a {} jour(s)) — seed ignoré",
            days_old
        );
    }

    // ── Seed graphe routier (indépendant — lancé si table vide) ──────────────
    if !graph_ready_persisted || edge_count == 0 {
        tracing::info!("Graphe routier vide — seed en arrière-plan…");
        let pool_bg   = pool.clone();
        let client_bg = http_client.clone();
        let ready_flag = routing_ready.clone();
        tokio::spawn(async move {
            match services::routing_graph::seed_routing_graph(&pool_bg, &client_bg).await {
                Err(e) => { tracing::warn!("Seed graphe routier échoué : {e}"); return; }
                Ok((n, e)) => tracing::info!("Graphe routier seedé : {n} nœuds, {e} arêtes"),
            }
            match services::routing_graph::compute_edge_exposures(&pool_bg).await {
                Err(e) => { tracing::warn!("Calcul exposition échoué : {e}"); return; }
                Ok(u)  => tracing::info!("Exposition calculée : {u} arêtes exposées"),
            }
            let _ = db::set_metadata(&pool_bg, "routing_graph_ready", "1").await;
            ready_flag.store(true, Ordering::Relaxed);
            tracing::info!("Routeur A* prêt ✓");
        });
    } else {
        tracing::info!("{edge_count} arêtes routières en base — seed ignoré");
    }

    routing_ready
}

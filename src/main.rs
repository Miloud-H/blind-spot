use axum::{
    http::{header, HeaderValue, Method},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::{env, str::FromStr};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

mod db;
mod error;
mod geo;
mod handlers;
mod models;
mod rate_limit;
mod route_cache;
mod routing;
mod services;

pub use error::AppError;
pub type AppResult<T> = Result<T, AppError>;

// ── État partagé entre les handlers ─────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub pool:        SqlitePool,
    pub http_client: reqwest::Client,
    pub config:      Config,
    pub route_cache: route_cache::RouteCache,
    pub event_bus:   tokio::sync::broadcast::Sender<String>,
}

#[derive(Clone)]
pub struct Config {
    /// URL Valhalla self-hosted — prioritaire sur ORS si défini (ex. http://localhost:8002)
    pub valhalla_url:  String,
    /// Clé API OpenRouteService — utilisée si VALHALLA_URL est vide
    pub ors_api_key:   String,
    pub admin_token:   String,
    pub port:          u16,
}

// ── Point d'entrée ───────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Charger .env si présent
    dotenvy::dotenv().ok();

    // Initialiser le logger
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_env("RUST_LOG")
                .add_directive("blindspot_api=debug".parse()?)
                .add_directive("tower_http=info".parse()?),
        )
        .init();

    // Configuration
    // DATABASE_URL : sqlite:./blindspot.db par défaut (fichier local, zéro config)
    let database_url = env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite:./blindspot.db".to_string());
    let valhalla_url = env::var("VALHALLA_URL").unwrap_or_default();
    let ors_api_key  = env::var("ORS_API_KEY").unwrap_or_default();
    let admin_token  = env::var("ADMIN_TOKEN").unwrap_or_default();
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .expect("PORT doit être un entier");

    // Pool SQLite — create_if_missing=true pour créer le fichier automatiquement
    tracing::info!("Ouverture SQLite : {database_url}");
    let opts = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(opts).await?;

    // Migrations
    tracing::info!("Application des migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("Migrations OK");

    if !valhalla_url.is_empty() {
        tracing::info!("Routing via Valhalla self-hosted : {valhalla_url}");
    } else if !ors_api_key.is_empty() {
        tracing::info!("Routing via ORS (API publique)");
    } else {
        tracing::warn!("VALHALLA_URL et ORS_API_KEY absents — endpoint /api/route désactivé");
    }

    // Client HTTP partagé (seed + routing)
    let http_client = reqwest::Client::new();

    // ── Auto-seed / re-seed Overpass ─────────────────────────────────────────
    // Conditions de (re-)seed :
    //   • Base OSM vide (premier lancement)
    //   • Dernier import OSM > 7 jours
    // Le seed est lancé en background — le serveur démarre immédiatement.
    let cam_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cameras WHERE source = 'osm'")
            .fetch_one(&pool)
            .await
            .unwrap_or(0);

    let days_old = db::days_since_osm_seed(&pool).await;
    let should_seed = cam_count == 0 || days_old >= 7;

    let bld_count: i64   = db::count_buildings(&pool).await;
    let edge_count: i64  = db::count_routing_edges(&pool).await;

    if should_seed {
        if cam_count == 0 {
            tracing::info!("Base OSM vide — import initial depuis Overpass en arrière-plan…");
        } else {
            tracing::info!(
                "Données OSM âgées de {} jour(s) — re-seed en arrière-plan…",
                days_old
            );
        }
        let pool_bg   = pool.clone();
        let client_bg = http_client.clone();
        tokio::spawn(async move {
            // 1. Caméras OSM (man_made=surveillance)
            match services::overpass::seed_from_overpass(&pool_bg, &client_bg).await {
                Ok(n)  => tracing::info!("Seed OSM terminé : {n} caméras importées/mises à jour"),
                Err(e) => tracing::warn!("Seed OSM échoué : {e}"),
            }
            // 2. Caméras déduites (métro STM + postes de police)
            match services::inferred::seed_inferred_cameras(&pool_bg, &client_bg).await {
                Ok(n)  => tracing::info!("Seed inféré terminé : {n} caméras déduites importées"),
                Err(e) => tracing::warn!("Seed inféré échoué : {e}"),
            }
            // 3. Bâtiments OSM pour le viewshed LOS (seulement si table vide)
            if bld_count == 0 {
                match services::buildings::seed_buildings(&pool_bg, &client_bg).await {
                    Ok(n)  => tracing::info!("Seed bâtiments terminé : {n} bâtiments insérés"),
                    Err(e) => tracing::warn!("Seed bâtiments échoué (routage en mode simplifié) : {e}"),
                }
            } else {
                tracing::info!("{bld_count} bâtiments déjà en base — seed ignoré");
            }
            // 4. Graphe routier piéton + exposition caméras (seulement si table vide)
            if edge_count == 0 {
                tracing::info!("Graphe routier vide — seed en arrière-plan...");
                match services::routing_graph::seed_routing_graph(&pool_bg, &client_bg).await {
                    Ok((n, e)) => {
                        tracing::info!("Graphe routier seedé : {n} nœuds, {e} arêtes");
                        match services::routing_graph::compute_edge_exposures(&pool_bg).await {
                            Ok(u)  => tracing::info!("Exposition calculée : {u} arêtes exposées"),
                            Err(e) => tracing::warn!("Calcul exposition échoué : {e}"),
                        }
                    }
                    Err(e) => tracing::warn!("Seed graphe routier échoué : {e}"),
                }
            } else {
                tracing::info!("{edge_count} arêtes routières en base — seed ignoré");
            }
        });
    } else {
        tracing::info!(
            "{cam_count} caméras OSM en base (import il y a {} jour(s)) — seed ignoré",
            days_old
        );
    }

    // Canal d'événements WebSocket (capacité 64 — les clients lents droppent des events, pas grave)
    let (event_tx, _) = tokio::sync::broadcast::channel::<String>(64);

    // Rate limiters : /api/route (10/min burst 3) et /api/cameras (60/min burst 10)
    let route_rl   = rate_limit::new_limiter(10,  3);
    let cameras_rl = rate_limit::new_limiter(60, 10);

    // Nettoyage des entrées IP inactives toutes les 10 minutes
    {
        let r = route_rl.clone();
        let c = cameras_rl.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(600));
            loop { tick.tick().await; r.retain_recent(); c.retain_recent(); }
        });
    }

    let config = Config { valhalla_url, ors_api_key, admin_token, port };
    let state = AppState {
        pool,
        http_client,
        config,
        route_cache: route_cache::RouteCache::new(),
        event_bus: event_tx,
    };

    // CORS — permissif pour le prototype (à restreindre en prod)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    // Fichiers statiques servis depuis ./public/
    let static_files = ServeDir::new("public");

    // Helper : construit un middleware de rate-limit à partir d'un limiter
    let rl = |lim: rate_limit::KeyedLimiter| {
        middleware::from_fn(move |req, next| {
            let lim = Arc::clone(&lim);
            async move { rate_limit::enforce(lim, req, next).await }
        })
    };

    // Routeur : API en premier, puis fallback vers les fichiers statiques
    let app = Router::new()
        .route("/", get(serve_index))
        .route("/admin.html", get(serve_admin))
        .route("/health", get(health))
        .route("/api/events", get(handlers::ws::events))
        .route(
            "/api/cameras",
            get(handlers::cameras::list)
                .post(handlers::cameras::create)
                .layer(rl(cameras_rl.clone())),
        )
        .route("/api/cameras/:id/report",
            post(handlers::cameras::report)
                .layer(rl(cameras_rl)))
        .route("/api/route",
            post(handlers::routing::calculate)
                .layer(rl(route_rl)))
        .route("/api/admin/stats",         get(handlers::admin::stats))
        .route("/api/admin/reports",       get(handlers::admin::list_reports))
        .route("/api/admin/cameras",       get(handlers::admin::list_cameras)
                                           .delete(handlers::admin::delete_cameras_bulk))
        .route("/api/admin/cameras/:id",   axum::routing::delete(handlers::admin::delete_camera)
                                           .patch(handlers::admin::update_camera))
        .route("/api/admin/zones",         get(handlers::admin::zones))
        .route("/api/admin/export/osm",    get(handlers::admin::export_osm))
        .route("/api/admin/cache",         axum::routing::delete(handlers::admin::clear_cache))
        .route("/api/admin/reseed",        post(handlers::admin::reseed))
        .with_state(state)
        .layer(cors)
        .fallback_service(static_files);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("BLINDSPOT API ▶ http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("Serveur arrêté proprement.");
    Ok(())
}

// ── Index HTML avec cache-busting ────────────────────────────────────────────
// Sert index.html avec :
//   • Cache-Control: no-cache — le browser revalide toujours (pas de cache intermédiaire)
//   • ?v=GIT_HASH sur les assets — URL unique par build → browser recharge si changé

const INDEX_HTML: &str = include_str!("../public/index.html");
const ADMIN_HTML: &str = include_str!("../public/admin.html");
const BUILD_ID:   &str = env!("GIT_HASH");

async fn serve_index() -> impl IntoResponse {
    let html = INDEX_HTML
        .replace(r#"href="/css/style.css""#,    &format!(r#"href="/css/style.css?v={}""#,    BUILD_ID))
        .replace(r#"src="/js/app.js""#,         &format!(r#"src="/js/app.js?v={}""#,         BUILD_ID))
        .replace(r#"src="/js/geo.js""#,         &format!(r#"src="/js/geo.js?v={}""#,         BUILD_ID))
        .replace(r#"src="/js/viewshed.js""#,    &format!(r#"src="/js/viewshed.js?v={}""#,    BUILD_ID))
        .replace(r#"src="/js/cameras.js""#,     &format!(r#"src="/js/cameras.js?v={}""#,     BUILD_ID))
        .replace(r#"src="/js/routing.js""#,     &format!(r#"src="/js/routing.js?v={}""#,     BUILD_ID))
        .replace(r#"src="/js/ui.js""#,          &format!(r#"src="/js/ui.js?v={}""#,          BUILD_ID));

    (
        [
            (header::CONTENT_TYPE,  HeaderValue::from_static("text/html; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-store, must-revalidate")),
            (header::PRAGMA,        HeaderValue::from_static("no-cache")),
            (header::EXPIRES,       HeaderValue::from_static("0")),
        ],
        html,
    )
}

async fn serve_admin() -> impl IntoResponse {
    let html = ADMIN_HTML
        .replace(r#"href="/css/admin.css""#, &format!(r#"href="/css/admin.css?v={}""#, BUILD_ID))
        .replace(r#"src="/js/admin.js""#,    &format!(r#"src="/js/admin.js?v={}""#,    BUILD_ID));
    (
        [
            (header::CONTENT_TYPE,  HeaderValue::from_static("text/html; charset=utf-8")),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-store, must-revalidate")),
            (header::PRAGMA,        HeaderValue::from_static("no-cache")),
            (header::EXPIRES,       HeaderValue::from_static("0")),
        ],
        html,
    )
}

// ── Health check ─────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "blindspot-api", "build": BUILD_ID }))
}

// ── Graceful shutdown ─────────────────────────────────────────────────────────
// Attend Ctrl+C (toutes plateformes) ou SIGTERM (Unix uniquement).

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Impossible d'installer le handler Ctrl+C");
    };

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Impossible d'installer le handler SIGTERM")
            .recv()
            .await;
    };

    // Sur Windows, SIGTERM n'existe pas — on attend uniquement Ctrl+C
    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c  => { tracing::info!("Ctrl+C reçu — arrêt en cours…"); }
        _ = sigterm => { tracing::info!("SIGTERM reçu — arrêt en cours…"); }
    }
}

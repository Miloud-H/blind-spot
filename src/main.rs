use axum::{
    http::{header, Method},
    routing::{get, post},
    Json, Router,
};
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::{env, str::FromStr};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

mod db;
mod error;
mod geo;
mod handlers;
mod models;
mod route_cache;
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
}

#[derive(Clone)]
pub struct Config {
    /// Clé API OpenRouteService — configurer via ORS_API_KEY dans .env
    pub ors_api_key: String,
    pub port: u16,
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
    // Clé API ORS — optionnelle si le routing est géré côté frontend
    let ors_api_key = env::var("ORS_API_KEY").unwrap_or_default();
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

    if ors_api_key.is_empty() {
        tracing::warn!("ORS_API_KEY non configurée — endpoint /api/route désactivé");
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
        });
    } else {
        tracing::info!(
            "{cam_count} caméras OSM en base (import il y a {} jour(s)) — seed ignoré",
            days_old
        );
    }

    let config = Config { ors_api_key, port };
    let state = AppState {
        pool,
        http_client,
        config,
        route_cache: route_cache::RouteCache::new(),
    };

    // CORS — permissif pour le prototype (à restreindre en prod)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    // Fichiers statiques servis depuis ./public/
    let static_files = ServeDir::new("public");

    // Routeur : API en premier, puis fallback vers les fichiers statiques
    let app = Router::new()
        .route("/health", get(health))
        .route(
            "/api/cameras",
            get(handlers::cameras::list).post(handlers::cameras::create),
        )
        .route("/api/route", post(handlers::routing::calculate))
        .route("/api/admin/seed", post(handlers::cameras::seed))
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

// ── Health check ─────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "blindspot-api" }))
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

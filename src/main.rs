use axum::{
    http::{header, HeaderValue, Method},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
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
mod startup;

pub use error::AppError;
pub type AppResult<T> = Result<T, AppError>;

// ── État partagé entre les handlers ─────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub pool:          SqlitePool,
    pub http_client:   reqwest::Client,
    pub config:        Config,
    pub route_cache:   route_cache::RouteCache,
    pub event_bus:     tokio::sync::broadcast::Sender<String>,
    /// true quand le graphe routier A* est complètement seedé et prêt.
    pub routing_ready: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct Config {
    /// Clé API OpenRouteService — fallback si graphe A* non prêt
    pub ors_api_key:  String,
    pub admin_token:  String,
    pub port:         u16,
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
    let ors_api_key = env::var("ORS_API_KEY").unwrap_or_default();
    let admin_token  = env::var("ADMIN_TOKEN").unwrap_or_default();
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .expect("PORT doit être un entier");

    // Pool SQLite — create_if_missing=true pour créer le fichier automatiquement
    tracing::info!("Ouverture SQLite : {database_url}");
    let opts = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .pragma("busy_timeout", "10000"); // attend jusqu'à 10s si DB verrouillée
    let pool = SqlitePool::connect_with(opts).await?;

    // Migrations
    tracing::info!("Application des migrations...");
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("Migrations OK");

    if !ors_api_key.is_empty() {
        tracing::info!("Fallback routing via ORS (API publique)");
    } else {
        tracing::warn!("ORS_API_KEY absent — fallback routing désactivé (A* requis)");
    }

    // Client HTTP partagé (seed + routing)
    let http_client = reqwest::Client::new();

    // Canal d'événements WebSocket (capacité 64 — les clients lents droppent des events, pas grave)
    // Créé avant les tâches de seed en arrière-plan pour qu'elles puissent diffuser les
    // suppressions de caméras obsolètes en temps réel (voir startup.rs).
    let (event_tx, _) = tokio::sync::broadcast::channel::<String>(64);

    // Tâches de seed (caméras OSM/inférées, bâtiments, graphe routier) — arrière-plan,
    // le serveur démarre immédiatement. Voir startup.rs.
    let routing_ready = startup::spawn_seed_tasks(&pool, &http_client, &event_tx).await;

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

    let config = Config { ors_api_key, admin_token, port };
    // routing_ready déjà initialisé plus haut
    let state = AppState {
        pool,
        http_client,
        config,
        route_cache:   route_cache::RouteCache::new(),
        event_bus:     event_tx,
        routing_ready,
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
        .merge(admin_routes(state.clone()))
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

/// Groupe des routes `/api/admin/*`, protégées par un seul middleware d'auth
/// (`route_layer`) plutôt qu'un appel `require_admin(...)` répété dans chaque handler.
fn admin_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/admin/stats",         get(handlers::admin::stats))
        .route("/api/admin/reports",       get(handlers::admin::list_reports))
        .route("/api/admin/duplicates",    get(handlers::admin::list_duplicates))
        .route("/api/admin/cameras/:id/dismiss-duplicate", post(handlers::admin::dismiss_duplicate))
        .route("/api/admin/cameras",       get(handlers::admin::list_cameras)
                                           .delete(handlers::admin::delete_cameras_bulk))
        .route("/api/admin/cameras/:id",   axum::routing::delete(handlers::admin::delete_camera)
                                           .patch(handlers::admin::update_camera))
        .route("/api/admin/export/osm",    get(handlers::admin::export_osm))
        .route("/api/admin/cache",         axum::routing::delete(handlers::admin::clear_cache))
        .route("/api/admin/reseed",        post(handlers::admin::reseed))
        .route("/api/admin/reseed-graph",  post(handlers::admin::reseed_graph))
        .route_layer(middleware::from_fn_with_state(state, handlers::admin::auth_middleware))
}

// ── Index HTML avec cache-busting ────────────────────────────────────────────
// Sert index.html avec :
//   • Cache-Control: no-cache — le browser revalide toujours (pas de cache intermédiaire)
//   • ?v=GIT_HASH sur les assets — URL unique par build → browser recharge si changé

const INDEX_HTML: &str = include_str!("../public/index.html");
const ADMIN_HTML: &str = include_str!("../public/admin.html");
const BUILD_ID:   &str = env!("GIT_HASH");

/// Ajoute `?v=build_id` à la fin de tout attribut `src="/js/...">` ou `href="/css/...">`.
/// Générique — aucune ligne à ajouter quand un nouveau module JS/CSS apparaît,
/// contrairement à une liste de `.replace()` par fichier (facile à oublier).
fn cache_bust(html: &str, build_id: &str) -> String {
    const MARKERS: [&str; 2] = ["src=\"/js/", "href=\"/css/"];
    let mut out = String::with_capacity(html.len() + 64);
    let mut rest = html;
    loop {
        let next = MARKERS.iter()
            .filter_map(|m| rest.find(m).map(|i| (i, *m)))
            .min_by_key(|&(i, _)| i);

        let Some((idx, marker)) = next else {
            out.push_str(rest);
            break;
        };

        let after_marker = idx + marker.len();
        out.push_str(&rest[..after_marker]);
        let tail = &rest[after_marker..];
        let end_quote = tail.find('"').unwrap_or(tail.len());
        out.push_str(&tail[..end_quote]);
        out.push_str("?v=");
        out.push_str(build_id);
        rest = &tail[end_quote..];
    }
    out
}

async fn serve_index() -> impl IntoResponse {
    let html = cache_bust(INDEX_HTML, BUILD_ID);
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
    let html = cache_bust(ADMIN_HTML, BUILD_ID);
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

#[cfg(test)]
mod cache_bust_tests {
    use super::cache_bust;

    #[test]
    fn appends_version_to_js_and_css_without_touching_other_attrs() {
        let html = r#"<link href="/css/style.css"><script src="/js/app.js"></script><img src="/img/logo.png">"#;
        let out = cache_bust(html, "abc123");
        assert!(out.contains(r#"href="/css/style.css?v=abc123""#));
        assert!(out.contains(r#"src="/js/app.js?v=abc123""#));
        // Une ressource hors /js /css n'est pas touchée
        assert!(out.contains(r#"src="/img/logo.png""#));
    }

    #[test]
    fn handles_multiple_js_files_without_per_file_code() {
        let html = r#"<script src="/js/a.js"></script><script src="/js/b.js"></script>"#;
        let out = cache_bust(html, "v1");
        assert_eq!(out.matches("?v=v1").count(), 2);
    }
}

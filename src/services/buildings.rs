use reqwest::Client;
use sqlx::SqlitePool;
use std::time::Duration;

const ENDPOINTS: &[&str] = &[
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass-api.de/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
];

/// Seed les bâtiments OSM de Montréal dans la table `buildings`.
/// Utilise `way[building]` avec `out geom qt` (geometry inline, tri quadtree).
/// Idempotent grâce à INSERT OR IGNORE.
pub async fn seed_buildings(pool: &SqlitePool, client: &Client) -> anyhow::Result<usize> {
    // Même bbox que le seed caméras
    let query = "[out:json][timeout:90];\
                 (way[building](45.45,-73.97,45.70,-73.47););\
                 out geom qt;";

    tracing::info!("Seed bâtiments Overpass...");

    let mut last_err: Option<anyhow::Error> = None;

    for (i, &endpoint) in ENDPOINTS.iter().enumerate() {
        if i > 0 { tokio::time::sleep(Duration::from_secs(3)).await; }
        tracing::debug!("Buildings — essai {endpoint}");

        let result = client
            .post(endpoint)
            .header("User-Agent", "BlindspotMTL/1.0 (https://github.com/blindspot-mtl)")
            .form(&[("data", query)])
            .timeout(Duration::from_secs(120))
            .send()
            .await;

        match result {
            Err(e) => { last_err = Some(e.into()); continue; }
            Ok(resp) if !resp.status().is_success() => {
                last_err = Some(anyhow::anyhow!("HTTP {}", resp.status()));
                continue;
            }
            Ok(resp) => {
                let text = resp.text().await?;
                let inserted = parse_and_store(&text, pool).await?;
                tracing::info!("Seed bâtiments : {inserted} bâtiments insérés");
                return Ok(inserted);
            }
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Tous les endpoints Overpass ont échoué")))
}

async fn parse_and_store(json: &str, pool: &SqlitePool) -> anyhow::Result<usize> {
    let data: serde_json::Value = serde_json::from_str(json)?;
    let elements = match data["elements"].as_array() {
        Some(e) => e,
        None => return Ok(0),
    };

    let mut inserted = 0usize;

    for el in elements {
        let osm_id = match el["id"].as_i64() { Some(id) => id, None => continue };

        let geometry = match el["geometry"].as_array() {
            Some(g) if g.len() >= 3 => g,
            _ => continue,
        };

        let pts: Vec<[f64; 2]> = geometry.iter().filter_map(|p| {
            Some([p["lat"].as_f64()?, p["lon"].as_f64()?])
        }).collect();
        if pts.len() < 3 { continue; }

        let (mut mn_lat, mut mx_lat, mut mn_lng, mut mx_lng) =
            (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        for &[la, lo] in &pts {
            if la < mn_lat { mn_lat = la; } if la > mx_lat { mx_lat = la; }
            if lo < mn_lng { mn_lng = lo; } if lo > mx_lng { mx_lng = lo; }
        }

        let geom_json = serde_json::to_string(&pts)?;

        let r = sqlx::query(
            "INSERT OR IGNORE INTO buildings \
             (osm_id, min_lat, max_lat, min_lng, max_lng, geom) \
             VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(osm_id)
        .bind(mn_lat).bind(mx_lat)
        .bind(mn_lng).bind(mx_lng)
        .bind(&geom_json)
        .execute(pool)
        .await?;

        if r.rows_affected() > 0 { inserted += 1; }
    }

    Ok(inserted)
}

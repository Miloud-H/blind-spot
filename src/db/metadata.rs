use sqlx::SqlitePool;

// ── Métadonnées ───────────────────────────────────────────────────────────────

/// Lit une valeur depuis la table `metadata`. Retourne `None` si la clé n'existe pas.
#[allow(dead_code)]
pub async fn get_metadata(pool: &SqlitePool, key: &str) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT value FROM metadata WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
}

/// Écrit (ou met à jour) une valeur dans `metadata`.
#[allow(dead_code)]
pub async fn set_metadata(pool: &SqlitePool, key: &str, value: &str) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO metadata (key, value, updated_at)
        VALUES ($1, $2, datetime('now'))
        ON CONFLICT(key) DO UPDATE SET
            value      = excluded.value,
            updated_at = datetime('now')
        "#,
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Enregistre l'horodatage du dernier import OSM (valeur = datetime SQLite).
/// Utilisé par le mécanisme de re-seed automatique.
pub async fn touch_seed_timestamp(pool: &SqlitePool) -> sqlx::Result<()> {
    sqlx::query(
        r#"
        INSERT INTO metadata (key, value, updated_at)
        VALUES ('osm_seeded_at', datetime('now'), datetime('now'))
        ON CONFLICT(key) DO UPDATE SET
            value      = datetime('now'),
            updated_at = datetime('now')
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Retourne le nombre de jours entiers depuis le dernier import OSM.
/// Retourne `i64::MAX` si aucun import n'a encore eu lieu.
pub async fn days_since_osm_seed(pool: &SqlitePool) -> i64 {
    let result: sqlx::Result<Option<Option<f64>>> = sqlx::query_scalar(
        "SELECT julianday('now') - julianday(value) FROM metadata WHERE key = 'osm_seeded_at'",
    )
    .fetch_optional(pool)
    .await;

    result
        .ok()
        .flatten()
        .flatten()
        .map(|d| d as i64)
        .unwrap_or(i64::MAX)
}

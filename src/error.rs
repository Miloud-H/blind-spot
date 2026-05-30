use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    Database(sqlx::Error),
    External(String),
    BadRequest(String),
    NotFound,
    Unauthorized,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::Database(e) => {
                tracing::error!("DB error: {e}");
                (StatusCode::INTERNAL_SERVER_ERROR, "Erreur base de données".to_string())
            }
            AppError::External(e) => {
                tracing::error!("Service externe: {e}");
                (StatusCode::BAD_GATEWAY, format!("Service externe indisponible: {e}"))
            }
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound       => (StatusCode::NOT_FOUND,        "Ressource introuvable".to_string()),
            AppError::Unauthorized   => (StatusCode::UNAUTHORIZED,     "Token invalide".to_string()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Database(e)
    }
}

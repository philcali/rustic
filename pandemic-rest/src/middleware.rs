use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{Json, Response},
};
use serde_json::json;

use crate::handlers::AppState;

pub async fn auth_middleware(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    // Extract API key from Authorization header
    let auth_header = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));

    let api_key = match auth_header {
        Some(key) => key,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(
                    json!({"status": "error", "message": "Missing or invalid Authorization header"}),
                ),
            ));
        }
    };

    // Authenticate and get scopes
    let scopes = match state.auth_config.authenticate(api_key) {
        Some(scopes) => scopes,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(json!({"status": "error", "message": "Invalid API key"})),
            ));
        }
    };

    // Add scopes to request extensions for handlers to use
    request.extensions_mut().insert(scopes);

    Ok(next.run(request).await)
}

use axum::{extract::State, http::StatusCode, response::Json, Extension};
use pandemic_common::DaemonClient;
use pandemic_protocol::{Request, Response as PandemicResponse};
use serde::Deserialize;
use serde_json::json;

use crate::handlers::{ApiResult, AppState};

#[derive(Deserialize)]
pub struct PublishEventRequest {
    pub topic: String,
    pub data: serde_json::Value,
}

pub async fn publish_event(
    State(state): State<AppState>,
    Extension(scopes): Extension<Vec<String>>,
    Json(payload): Json<PublishEventRequest>,
) -> ApiResult {
    if !state.auth_config.authorize(&scopes, "events:publish") {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"status": "error", "message": "Insufficient permissions"})),
        ));
    }

    let request = Request::Publish {
        topic: payload.topic,
        data: payload.data,
    };

    match DaemonClient::send_request(&state.socket_path, &request).await {
        Ok(PandemicResponse::Success { data }) => {
            Ok(Json(json!({"status": "success", "data": data})))
        }
        Ok(PandemicResponse::Error { message }) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "message": message})),
        )),
        Ok(PandemicResponse::NotFound { message }) => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"status": "not_found", "message": message})),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(
                json!({"status": "error", "message": format!("Daemon communication error: {}", e)}),
            ),
        )),
    }
}

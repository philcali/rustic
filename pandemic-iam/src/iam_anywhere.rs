use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionResponse {
    pub credential_set: Vec<CredentialSet>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialSet {
    pub credentials: Credentials,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expiration: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub duration_seconds: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_session_name: Option<String>,
}

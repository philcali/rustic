use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

mod time_format {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn serialize<S>(time: &Option<SystemTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match time {
            Some(t) => {
                let duration = t.duration_since(UNIX_EPOCH).unwrap();
                let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(duration.as_secs() as i64, 0)
                    .unwrap()
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string();
                serializer.serialize_str(&datetime)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let opt: Option<String> = Option::deserialize(deserializer)?;
        match opt {
            Some(_) => Ok(Some(SystemTime::now())), // Simplified for now
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub description: Option<String>,
    pub config: Option<HashMap<String, String>>,
    #[serde(with = "time_format")]
    pub registered_at: Option<SystemTime>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Register { plugin: PluginInfo },
    ListPlugins,
    GetPlugin { name: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status")]
pub enum Response {
    Success { data: Option<serde_json::Value> },
    Error { message: String },
    NotFound { message: String },
}

impl Response {
    pub fn success() -> Self {
        Self::Success { data: None }
    }

    pub fn success_with_data(data: serde_json::Value) -> Self {
        Self::Success { data: Some(data) }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error { message: message.into() }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound { message: message.into() }
    }
}
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
    pub version: String,
    pub description: Option<String>,
    pub config: Option<HashMap<String, String>>,
    #[serde(with = "time_format")]
    pub registered_at: Option<SystemTime>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    Register { plugin: PluginInfo },
    Deregister { name: String },
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;
    use std::collections::HashMap;

    #[test]
    fn test_plugin_info_serialization() {
        let mut config = HashMap::new();
        config.insert("key1".to_string(), "value1".to_string());
        
        let plugin = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test description".to_string()),
            config: Some(config),
            registered_at: None,
        };
        
        let json = serde_json::to_string(&plugin).unwrap();
        let deserialized: PluginInfo = serde_json::from_str(&json).unwrap();
        
        assert_eq!(plugin.name, deserialized.name);
        assert_eq!(plugin.description, deserialized.description);
        assert_eq!(plugin.config, deserialized.config);
    }

    #[test]
    fn test_register_request_serialization() {
        let plugin = PluginInfo {
            name: "test-plugin".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: None,
        };
        
        let request = Request::Register { plugin };
        let json = serde_json::to_string(&request).unwrap();
        
        assert!(json.contains(r#""type":"Register""#));
        assert!(json.contains(r#""name":"test-plugin""#));
        assert!(json.contains(r#""version":"1.0.0""#));
        
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::Register { plugin } => assert_eq!(plugin.name, "test-plugin"),
            _ => panic!("Expected Register request"),
        }
    }

    #[test]
    fn test_deregister_request_serialization() {
        let request = Request::Deregister { name: "test-plugin".to_string() };
        let json = serde_json::to_string(&request).unwrap();
        
        assert!(json.contains(r#""type":"Deregister""#));
        assert!(json.contains(r#""name":"test-plugin""#));
        
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::Deregister { name } => assert_eq!(name, "test-plugin"),
            _ => panic!("Expected Deregister request"),
        }
    }

    #[test]
    fn test_list_plugins_request_serialization() {
        let request = Request::ListPlugins;
        let json = serde_json::to_string(&request).unwrap();
        
        assert_eq!(json, r#"{"type":"ListPlugins"}"#);
        
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::ListPlugins => {},
            _ => panic!("Expected ListPlugins request"),
        }
    }

    #[test]
    fn test_get_plugin_request_serialization() {
        let request = Request::GetPlugin { name: "test-plugin".to_string() };
        let json = serde_json::to_string(&request).unwrap();
        
        assert!(json.contains(r#""type":"GetPlugin""#));
        assert!(json.contains(r#""name":"test-plugin""#));
        
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        match deserialized {
            Request::GetPlugin { name } => assert_eq!(name, "test-plugin"),
            _ => panic!("Expected GetPlugin request"),
        }
    }

    #[test]
    fn test_success_response_serialization() {
        let response = Response::success();
        let json = serde_json::to_string(&response).unwrap();
        
        assert!(json.contains(r#""status":"Success""#));
        
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::Success { data } => assert!(data.is_none()),
            _ => panic!("Expected Success response"),
        }
    }

    #[test]
    fn test_success_with_data_response_serialization() {
        let data = serde_json::json!({"test": "value"});
        let response = Response::success_with_data(data.clone());
        let json = serde_json::to_string(&response).unwrap();
        
        assert!(json.contains(r#""status":"Success""#));
        
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::Success { data: Some(d) } => assert_eq!(d, data),
            _ => panic!("Expected Success response with data"),
        }
    }

    #[test]
    fn test_error_response_serialization() {
        let response = Response::error("Test error");
        let json = serde_json::to_string(&response).unwrap();
        
        assert!(json.contains(r#""status":"Error""#));
        assert!(json.contains(r#""message":"Test error""#));
        
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::Error { message } => assert_eq!(message, "Test error"),
            _ => panic!("Expected Error response"),
        }
    }

    #[test]
    fn test_not_found_response_serialization() {
        let response = Response::not_found("Plugin not found");
        let json = serde_json::to_string(&response).unwrap();
        
        assert!(json.contains(r#""status":"NotFound""#));
        assert!(json.contains(r#""message":"Plugin not found""#));
        
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        match deserialized {
            Response::NotFound { message } => assert_eq!(message, "Plugin not found"),
            _ => panic!("Expected NotFound response"),
        }
    }

    #[test]
    fn test_timestamp_serialization() {
        let plugin = PluginInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            description: None,
            config: None,
            registered_at: Some(SystemTime::now()),
        };
        
        let json = serde_json::to_string(&plugin).unwrap();
        assert!(json.contains("UTC"));
        
        // Should deserialize without error
        let _: PluginInfo = serde_json::from_str(&json).unwrap();
    }
}
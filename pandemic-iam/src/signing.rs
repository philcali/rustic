use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue};
use sha2::{Digest, Sha256};

use crate::signer::FileSigner;
use std::collections::BTreeMap;

pub struct SigningParams {
    pub region: String,
    pub service: String,
    pub algorithm: String,
    pub timestamp: DateTime<Utc>,
}

impl SigningParams {
    pub fn new(region: String) -> Self {
        Self {
            region,
            service: "rolesanywhere".to_string(),
            algorithm: "AWS4-X509-RSA-SHA256".to_string(), // Default to RSA
            timestamp: Utc::now(),
        }
    }

    pub fn formatted_timestamp(&self) -> String {
        self.timestamp.format("%Y%m%dT%H%M%SZ").to_string()
    }

    pub fn date_stamp(&self) -> String {
        self.timestamp.format("%Y%m%d").to_string()
    }

    pub fn credential_scope(&self) -> String {
        format!(
            "{}/{}/{}/aws4_request",
            self.date_stamp(),
            self.region,
            self.service
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub fn sign_request(
    method: &str,
    uri: &str,
    headers: &mut HeaderMap,
    body: &str,
    params: &SigningParams,
    certificate_b64: &str,
    serial_number: &str,
    signer: &FileSigner,
) -> Result<()> {
    // Add required headers
    headers.insert("host", HeaderValue::from_str(&extract_host_from_uri(uri)?)?);
    headers.insert(
        "x-amz-date",
        HeaderValue::from_str(&params.formatted_timestamp())?,
    );
    headers.insert("x-amz-x509", HeaderValue::from_str(certificate_b64)?);

    // Create canonical request
    let canonical_request = create_canonical_request(method, uri, headers, body)?;
    let canonical_request_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));

    // Create string to sign
    let string_to_sign = create_string_to_sign(params, &canonical_request_hash);

    // Sign the string to sign using RSA PKCS1v15 with SHA256
    let signature_bytes = signer.sign_string_to_sign(&string_to_sign)?;
    let signature = hex::encode(signature_bytes);

    // Create authorization header
    let signed_headers = get_signed_headers(headers);
    let credential = format!("{}/{}", serial_number, params.credential_scope());
    let auth_header = format!(
        "{} Credential={}, SignedHeaders={}, Signature={}",
        params.algorithm, credential, signed_headers, signature
    );

    headers.insert("authorization", HeaderValue::from_str(&auth_header)?);
    Ok(())
}

fn extract_host_from_uri(uri: &str) -> Result<String> {
    let url = reqwest::Url::parse(uri)?;
    url.host_str()
        .map(|h| h.to_string())
        .ok_or_else(|| anyhow!("No host in URI"))
}

fn hash_payload(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes())).to_lowercase()
}

fn create_canonical_request(
    method: &str,
    uri: &str,
    headers: &HeaderMap,
    body: &str,
) -> Result<String> {
    let url = reqwest::Url::parse(uri)?;
    let path = url.path();
    let query = url.query().unwrap_or("");

    let canonical_headers = create_canonical_headers(headers);
    let signed_headers = get_signed_headers(headers);
    let payload_hash = hash_payload(body);

    Ok(format!(
        "{}\n{}\n{}\n{}\n\n{}\n{}",
        method, path, query, canonical_headers, signed_headers, payload_hash
    ))
}

fn create_canonical_headers(headers: &HeaderMap) -> String {
    let mut canonical: BTreeMap<String, String> = BTreeMap::new();

    for (name, value) in headers {
        let name_str = name.as_str().to_lowercase();
        if !should_ignore_header(&name_str) {
            let value_str = value.to_str().unwrap_or("").trim();
            canonical.insert(name_str, value_str.to_string());
        }
    }

    canonical
        .iter()
        .map(|(k, v)| format!("{}:{}", k, v))
        .collect::<Vec<_>>()
        .join("\n")
}

fn get_signed_headers(headers: &HeaderMap) -> String {
    let mut signed: Vec<String> = headers
        .keys()
        .filter_map(|name| {
            let name_str = name.as_str().to_lowercase();
            if !should_ignore_header(&name_str) {
                Some(name_str)
            } else {
                None
            }
        })
        .collect();

    signed.sort();
    signed.join(";")
}

fn should_ignore_header(name: &str) -> bool {
    matches!(name, "authorization" | "user-agent" | "x-amzn-trace-id")
}

fn create_string_to_sign(params: &SigningParams, canonical_request_hash: &str) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        params.algorithm,
        params.formatted_timestamp(),
        params.credential_scope(),
        canonical_request_hash
    )
}

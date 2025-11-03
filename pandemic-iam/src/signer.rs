use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine};
use rsa::{pkcs1v15::Pkcs1v15Sign, pkcs8::DecodePrivateKey, RsaPrivateKey};
use rustls_pemfile::{certs, pkcs8_private_keys};
use sha2::{Digest, Sha256};
use std::fs;
use x509_parser::prelude::*;

pub struct FileSigner {
    pub certificate_der: Vec<u8>,
    pub rsa_key: Option<RsaPrivateKey>,
}

impl FileSigner {
    pub fn new(cert_path: &str, key_path: &str) -> Result<Self> {
        // Load certificate
        let cert_pem = fs::read_to_string(cert_path)?;
        let mut cert_reader = cert_pem.as_bytes();
        let cert_der = certs(&mut cert_reader)?;

        if cert_der.is_empty() {
            return Err(anyhow!("No certificate found"));
        }

        // Load private key
        let key_pem = fs::read_to_string(key_path)?;
        let mut key_reader = key_pem.as_bytes();
        let private_keys = pkcs8_private_keys(&mut key_reader)?;

        if private_keys.is_empty() {
            return Err(anyhow!("No private key found"));
        }

        // Try to parse RSA private key
        let rsa_key = RsaPrivateKey::from_pkcs8_der(&private_keys[0]).ok();

        Ok(FileSigner {
            certificate_der: cert_der[0].clone(),
            rsa_key,
        })
    }

    pub fn certificate_base64(&self) -> String {
        general_purpose::STANDARD.encode(&self.certificate_der)
    }

    pub fn get_serial_number(&self) -> Result<String> {
        let (_, cert) = X509Certificate::from_der(&self.certificate_der)
            .map_err(|e| anyhow!("Failed to parse certificate: {}", e))?;
        Ok(cert.serial.to_str_radix(10))
    }

    pub fn sign_string_to_sign(&self, string_to_sign: &str) -> Result<Vec<u8>> {
        if let Some(rsa_key) = &self.rsa_key {
            // Hash the string to sign with SHA256
            let mut hasher = Sha256::new();
            hasher.update(string_to_sign.as_bytes());
            let hash = hasher.finalize();

            // Sign with PKCS1v15 padding and SHA256 (with proper ASN.1 DigestInfo)
            let padding = Pkcs1v15Sign::new::<Sha256>();
            let signature = rsa_key
                .sign(padding, &hash)
                .map_err(|e| anyhow!("Failed to sign: {}", e))?;

            Ok(signature)
        } else {
            Err(anyhow!("RSA key not available for signing"))
        }
    }
}

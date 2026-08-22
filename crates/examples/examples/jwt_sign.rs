#!/usr/bin/env rust

// An ES256 JWT for the App Store Connect API with a throwaway key. The signature is random per
// run, so only its length is printed.

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::Serialize;

const KEY_PEM: &str = r"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgUjsMS1G++73N9gu/
+5DqPX72BGB3261f/6fVgua/j02hRANCAARXrcSO4Xy9nDWLnLrV8nO/rlYHRXXh
xIiAZsCMcX/+m6ARVQWOWv/+nOeIBDopELzaXHq8H0Sq7+hxVp4XlrqV
-----END PRIVATE KEY-----
";

#[derive(Serialize)]
struct Claims {
    iss: String,
    iat: i64,
    exp: i64,
    aud: String,
}

fn main() -> anyhow::Result<()> {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some("TESTKEY123".to_string());

    let claims = Claims {
        iss: "issuer-id-1".to_string(),
        iat: 1_700_000_000,
        exp: 1_700_001_200,
        aud: "appstoreconnect-v1".to_string(),
    };

    let key = EncodingKey::from_ec_pem(KEY_PEM.as_bytes())?;
    let token = encode(&header, &claims, &key)?;

    let parts: Vec<&str> = token.split('.').collect();
    println!("parts {}", parts.len());
    println!("header {}", parts[0]);
    println!("claims {}", parts[1]);
    println!("sig len {}", parts[2].len());
    Ok(())
}

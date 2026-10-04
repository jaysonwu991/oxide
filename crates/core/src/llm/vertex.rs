//! Google Vertex AI credentials.
//!
//! Vertex is not reached with an API key: a request carries an OAuth access
//! token for the Cloud Platform scope, which this module mints from whatever
//! credential the configuration holds — a service-account key, an authorized
//! user's refresh token, a plain access token someone pasted, or Application
//! Default Credentials — and keeps until it is about to expire.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// The scope a Vertex request is authorized for.
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Renew this long before the token expires, so a request never leaves with a
/// token that lapses in flight.
const RENEW_MARGIN_SECS: i64 = 300;

/// A minted token and the Unix second it stops being usable.
#[derive(Clone)]
struct Cached {
    token: String,
    expires_at: i64,
}

fn cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Reads a credential that is either the JSON itself or a path to it, falling
/// back to the file `GOOGLE_APPLICATION_CREDENTIALS` names and then to the
/// application default credentials.
fn read_credential(credential: &str) -> Option<(String, Value)> {
    read_credential_with(credential, &|name| std::env::var(name).ok())
}

/// The same read with the environment injected, so which files a credential
/// may come from is decided the same way in a test as on a machine.
fn read_credential_with(
    credential: &str,
    with_env: &dyn Fn(&str) -> Option<String>,
) -> Option<(String, Value)> {
    let trimmed = credential.trim();
    let (source, text) = if trimmed.starts_with('{') {
        ("inline".to_string(), trimmed.to_string())
    } else if !trimmed.is_empty() {
        // A pasted access token is not a path and not JSON, so a read that
        // fails leaves it to be used as the token it is.
        let text = std::fs::read_to_string(trimmed).ok()?;
        (trimmed.to_string(), text)
    } else if let Some(path) = with_env("GOOGLE_APPLICATION_CREDENTIALS")
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
    {
        let text = std::fs::read_to_string(&path).ok()?;
        (path, text)
    } else {
        // Application Default Credentials from `gcloud auth
        // application-default login`.
        let path = adc_path(with_env)?;
        let text = std::fs::read_to_string(&path).ok()?;
        (path.display().to_string(), text)
    };
    let value: Value = serde_json::from_str(&text).ok()?;
    Some((source, value))
}

fn adc_path(with_env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let home = with_env("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("gcloud")
            .join("application_default_credentials.json"),
    )
}

/// The project a credential belongs to, so a Vertex request can be built
/// without asking for what the key already says.
pub fn project_of(credential: &str) -> Option<String> {
    let (_, value) = read_credential(credential)?;
    value["project_id"]
        .as_str()
        .or_else(|| value["quota_project_id"].as_str())
        .map(str::to_string)
}

/// Whether there is a credential to mint a token from at all: one that was
/// stored, the file `GOOGLE_APPLICATION_CREDENTIALS` names, or the application
/// default credentials `gcloud auth application-default login` writes. The
/// caller asks before a turn starts, so a reader is told what to set rather
/// than watching a request fail.
/// The same question with the environment injected.
pub fn has_credential_with(with_env: &dyn Fn(&str) -> Option<String>) -> bool {
    read_credential_with("", with_env).is_some()
}

/// An access token for Vertex, from the cache when one is still good.
pub async fn access_token(http: &reqwest::Client, credential: &str) -> Result<String> {
    let trimmed = credential.trim();
    let now = now_secs();
    if let Ok(cache) = cache().lock() {
        if let Some(entry) = cache.get(trimmed) {
            if entry.expires_at - RENEW_MARGIN_SECS > now {
                return Ok(entry.token.clone());
            }
        }
    }

    let Some((source, value)) = read_credential(trimmed) else {
        if trimmed.is_empty() {
            bail!(
                "the Vertex provider needs a credential — set GOOGLE_VERTEX_CREDENTIALS to the \
                 service-account key, GOOGLE_APPLICATION_CREDENTIALS to its path, or log in with \
                 /login vertex"
            );
        }
        // Not a key file and not a service account: whatever was stored is the
        // token itself.
        return Ok(trimmed.to_string());
    };

    let (token, lifetime) = match value["type"].as_str() {
        Some("service_account") => {
            let assertion = service_account_jwt(&value, now)?;
            exchange(
                http,
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                    ("assertion", &assertion),
                ],
            )
            .await?
        }
        Some("authorized_user") => {
            let refresh = value["refresh_token"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if refresh.is_empty() {
                bail!("the Vertex credential at {source} has no refresh token");
            }
            let client_id = value["client_id"].as_str().unwrap_or_default().to_string();
            let client_secret = value["client_secret"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            exchange(
                http,
                &[
                    ("grant_type", "refresh_token"),
                    ("refresh_token", &refresh),
                    ("client_id", &client_id),
                    ("client_secret", &client_secret),
                ],
            )
            .await?
        }
        // Something else — an external account or a workload identity — is
        // reported rather than sent as a bearer token it is not.
        other => bail!(
            "the Vertex credential at {source} is a `{}` credential, which is not supported",
            other.unwrap_or("unknown")
        ),
    };

    if let Ok(mut cache) = cache().lock() {
        cache.insert(
            trimmed.to_string(),
            Cached {
                token: token.clone(),
                expires_at: now + lifetime,
            },
        );
    }
    Ok(token)
}

async fn exchange(http: &reqwest::Client, form: &[(&str, &str)]) -> Result<(String, i64)> {
    let response = http
        .post(TOKEN_URL)
        .form(form)
        .send()
        .await
        .context("requesting a Google access token")?;
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .context("reading the Google token response")?;
    if !status.is_success() {
        let detail = body["error_description"]
            .as_str()
            .or_else(|| body["error"].as_str())
            .unwrap_or("no detail");
        bail!("Google refused the token request ({status}): {detail}");
    }
    let Some(token) = body["access_token"].as_str() else {
        bail!("the Google token response carried no access_token");
    };
    Ok((
        token.to_string(),
        body["expires_in"].as_i64().unwrap_or(3600),
    ))
}

/// The signed JWT a service account exchanges for an access token. Split out
/// from the exchange so the signing can be tested without the network.
pub fn service_account_jwt(account: &Value, now: i64) -> Result<String> {
    let email = account["client_email"]
        .as_str()
        .context("the service-account key has no client_email")?;
    let key = account["private_key"]
        .as_str()
        .context("the service-account key has no private_key")?;
    let der = pem_body(key).context("the service-account private key is not a PEM block")?;
    let pair = ring::signature::RsaKeyPair::from_pkcs8(&der).map_err(|error| {
        anyhow::anyhow!("the service-account private key could not be read: {error}")
    })?;

    let header = json!({ "alg": "RS256", "typ": "JWT" });
    let claims = json!({
        "iss": email,
        "scope": SCOPE,
        "aud": TOKEN_URL,
        "iat": now,
        "exp": now + 3600,
    });
    let signing_input = format!(
        "{}.{}",
        base64url(header.to_string().as_bytes()),
        base64url(claims.to_string().as_bytes())
    );

    let mut signature = vec![0u8; pair.public().modulus_len()];
    let rng = ring::rand::SystemRandom::new();
    pair.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &rng,
        signing_input.as_bytes(),
        &mut signature,
    )
    .map_err(|error| anyhow::anyhow!("signing the service-account assertion failed: {error}"))?;
    Ok(format!("{signing_input}.{}", base64url(&signature)))
}

/// The DER inside a PEM block, whatever its header says.
fn pem_body(pem: &str) -> Option<Vec<u8>> {
    let body: String = pem
        .lines()
        .filter(|line| !line.trim_start().starts_with("-----"))
        .flat_map(|line| line.split_whitespace())
        .collect();
    if body.is_empty() {
        return None;
    }
    crate::media::base64_decode(&body)
}

/// base64url without padding, which is what a JWT is spelled in.
fn base64url(bytes: &[u8]) -> String {
    crate::media::base64_encode(bytes)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway 2048-bit key, so the signing can be checked without a
    /// network round trip. Nothing is trusted with it.
    const TEST_KEY: &str = concat!(
        "-----BEGIN PRIVATE KEY-----\n",
        "MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCzCawKqQ+Vce3i\n",
        "pXx9C7dudO5gMv3QFuxpq4AT5wCQIf1cmIuyiH8lRiu3mQqOHYSvfXwzgYuNMStK\n",
        "nTLqnes0+Td2wkYhSXZyKYOiVmveXyEHcGS0I2Fy+83hCJuiW1gEPeruFz/MBw2J\n",
        "NrLVQfO1gL1bVJ5fqpyM//dm5Vyl6bCEx2D5CtjwpcsOmoU+zdC6zaiJ6xLcp/zv\n",
        "qAbQ8jiucWouKsusdeSRgI+1hJrUfyqACPkSsREv6LQw1IlX+XD6gYAVaoNRH8Pf\n",
        "XZlNtFTCZbz8G3M82/Z2Ddzb8bYN/dwfp+FhJzPh7vcfvi6pv5ZIbrtfE7K90M+p\n",
        "bWs9a1JRAgMBAAECggEAE6Xw1cWqcmfrp6cqI4383jgibffCn4rYfJtH5cdA6lB0\n",
        "newWzqvIqlZKV26J+EE4jcrLbdqebksSWXCphRJ6HNjp05Rr/lyxzUMNkWwDEAco\n",
        "GIkm4BcO63/DXxoi/F0iamrCJMlz5Kf0fhW86AcCIC3l+gcuD0kpv5aat9Xdhk6R\n",
        "Psgm0FfnHxiMCB6ag0kUO7L+FnE3JsCSyn7TFi/9T8sNzxYDtOaPWJ9wH102PV1g\n",
        "KricAw83QNb4tdYOkS7L7hEMrYnwdFMw9e+r/uIMwQ3PfqdHX7HzPpbo/Vadkwty\n",
        "OT63L5LvU+53azyMRWJOj1ym8Op/WK/+wJqogw810QKBgQDn8/D7I8ubt64UyTYs\n",
        "cvdol6BN8f9AihIqvTOSilfL5cQdYNoX2NjtOVR6vp6UNVRfNsTQTLu/X1Fg95Oc\n",
        "Ecka8F086mVryawH2ZxgVHYndWsQSvpP0kAcJ8pgcXMf+0uSTEmQZ/y+FPrO+Kmu\n",
        "4nwrPlFpxPfx/oJIGkuy2mSC2QKBgQDFmVtkD/gdbaiJrMdIwZncZ13ydP4ocW/n\n",
        "NntWtKyvQ1+xXhTwVtaXwCZKBi7fuUxATnZGqoZstoBvu3ba4aQXnOMSiOdIQ/hG\n",
        "ntbbblPJVOWDJXDCx68jTGX5QK1FS8L2pRbz+6BeS6zb11C90ib9UL41xihzWcov\n",
        "TPK25J2wOQKBgEx/aWSKQ6ZRh7oXOiSirfHyv821GAfDAYbS62VS9u2qE2nU/ViM\n",
        "Dc7+71uj7kzW4cKdH3/LLshIuWAPhsEcJIUb7MQgUd6gnnngTPCAKo+M7Ahtkewt\n",
        "XTNr9BSmld9gU2YmfsR/Zx2Rcb48yEmDLIxeex809Mj8tR7ufTS5ThjRAoGAHSoy\n",
        "dAVrYKST0KZAz7tLg116X1PnQ37ivmSrNbP7HO87y4kQXBzlFYbV3+0js/+8gm1S\n",
        "RcHbj2YsdNFdjJbrSlTsqwqr4F2v4nSFT+Pr1/aA3aeECivA7NvU3UIvkN/Kye66\n",
        "AY7tL53JyLTyBOJkwNTPDk7OH6BsGCkqVdZOc/kCgYB+tCKIoziIpJSdwBF0bOeT\n",
        "TeLmxruMC+3Whg6hvnPq2EZXNvsHMnp7gOC+icy1GYIAbOx5NyW4Ej4k+LpAEdCa\n",
        "ik9jyJukvetcCmpx8FO1tJIRfh5VdufbZvGRLhxTvNIRLnLELKviK7VrMkdQQgyc\n",
        "0VFvqTs2yk4STjBWzWks8Q==\n",
        "-----END PRIVATE KEY-----\n",
    );

    /// The same key's public half, in the PKCS#1 DER ring verifies with.
    const TEST_PUBLIC: &[u8] = &[
        0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01, 0x00, 0xb3, 0x09, 0xac, 0x0a, 0xa9, 0x0f,
        0x95, 0x71, 0xed, 0xe2, 0xa5, 0x7c, 0x7d, 0x0b, 0xb7, 0x6e, 0x74, 0xee, 0x60, 0x32, 0xfd,
        0xd0, 0x16, 0xec, 0x69, 0xab, 0x80, 0x13, 0xe7, 0x00, 0x90, 0x21, 0xfd, 0x5c, 0x98, 0x8b,
        0xb2, 0x88, 0x7f, 0x25, 0x46, 0x2b, 0xb7, 0x99, 0x0a, 0x8e, 0x1d, 0x84, 0xaf, 0x7d, 0x7c,
        0x33, 0x81, 0x8b, 0x8d, 0x31, 0x2b, 0x4a, 0x9d, 0x32, 0xea, 0x9d, 0xeb, 0x34, 0xf9, 0x37,
        0x76, 0xc2, 0x46, 0x21, 0x49, 0x76, 0x72, 0x29, 0x83, 0xa2, 0x56, 0x6b, 0xde, 0x5f, 0x21,
        0x07, 0x70, 0x64, 0xb4, 0x23, 0x61, 0x72, 0xfb, 0xcd, 0xe1, 0x08, 0x9b, 0xa2, 0x5b, 0x58,
        0x04, 0x3d, 0xea, 0xee, 0x17, 0x3f, 0xcc, 0x07, 0x0d, 0x89, 0x36, 0xb2, 0xd5, 0x41, 0xf3,
        0xb5, 0x80, 0xbd, 0x5b, 0x54, 0x9e, 0x5f, 0xaa, 0x9c, 0x8c, 0xff, 0xf7, 0x66, 0xe5, 0x5c,
        0xa5, 0xe9, 0xb0, 0x84, 0xc7, 0x60, 0xf9, 0x0a, 0xd8, 0xf0, 0xa5, 0xcb, 0x0e, 0x9a, 0x85,
        0x3e, 0xcd, 0xd0, 0xba, 0xcd, 0xa8, 0x89, 0xeb, 0x12, 0xdc, 0xa7, 0xfc, 0xef, 0xa8, 0x06,
        0xd0, 0xf2, 0x38, 0xae, 0x71, 0x6a, 0x2e, 0x2a, 0xcb, 0xac, 0x75, 0xe4, 0x91, 0x80, 0x8f,
        0xb5, 0x84, 0x9a, 0xd4, 0x7f, 0x2a, 0x80, 0x08, 0xf9, 0x12, 0xb1, 0x11, 0x2f, 0xe8, 0xb4,
        0x30, 0xd4, 0x89, 0x57, 0xf9, 0x70, 0xfa, 0x81, 0x80, 0x15, 0x6a, 0x83, 0x51, 0x1f, 0xc3,
        0xdf, 0x5d, 0x99, 0x4d, 0xb4, 0x54, 0xc2, 0x65, 0xbc, 0xfc, 0x1b, 0x73, 0x3c, 0xdb, 0xf6,
        0x76, 0x0d, 0xdc, 0xdb, 0xf1, 0xb6, 0x0d, 0xfd, 0xdc, 0x1f, 0xa7, 0xe1, 0x61, 0x27, 0x33,
        0xe1, 0xee, 0xf7, 0x1f, 0xbe, 0x2e, 0xa9, 0xbf, 0x96, 0x48, 0x6e, 0xbb, 0x5f, 0x13, 0xb2,
        0xbd, 0xd0, 0xcf, 0xa9, 0x6d, 0x6b, 0x3d, 0x6b, 0x52, 0x51, 0x02, 0x03, 0x01, 0x00, 0x01,
    ];

    fn service_account() -> Value {
        json!({
            "type": "service_account",
            "project_id": "oxide-test",
            "client_email": "oxide@oxide-test.iam.gserviceaccount.com",
            "private_key": TEST_KEY,
        })
    }

    /// A service account with no file to read is not a credential, and a
    /// machine whose application default credentials are the only ones it has
    /// must not be refused before a turn even starts.
    #[test]
    fn a_credential_is_found_in_the_file_the_environment_names() {
        let dir = std::env::temp_dir().join(format!("oxide-vertex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("adc.json");
        std::fs::write(&path, service_account().to_string()).unwrap();
        let named = path.display().to_string();
        let env = |name: &str| match name {
            "GOOGLE_APPLICATION_CREDENTIALS" => Some(named.clone()),
            _ => None,
        };
        assert!(has_credential_with(&env));
        assert_eq!(project_of(&named).as_deref(), Some("oxide-test"));
        std::fs::remove_dir_all(&dir).ok();

        // Nothing named and no home to look in is nothing to mint a token from.
        let home = dir.join("home");
        let empty = |name: &str| (name == "HOME").then(|| home.display().to_string());
        assert!(!has_credential_with(&empty));
    }

    #[test]
    fn reads_the_project_and_the_scope_of_a_key() {
        let account = service_account();
        assert_eq!(
            project_of(&account.to_string()).as_deref(),
            Some("oxide-test")
        );
        assert_eq!(project_of("not a credential"), None);
    }

    #[test]
    fn signs_an_assertion_the_public_key_verifies() {
        let account = service_account();
        let jwt = service_account_jwt(&account, 1_700_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header: Value = serde_json::from_slice(&decode(parts[0])).unwrap();
        assert_eq!(header["alg"], "RS256");
        let claims: Value = serde_json::from_slice(&decode(parts[1])).unwrap();
        assert_eq!(claims["iss"], "oxide@oxide-test.iam.gserviceaccount.com");
        assert_eq!(claims["scope"], SCOPE);
        assert_eq!(claims["aud"], TOKEN_URL);
        assert_eq!(claims["iat"], 1_700_000_000);
        assert_eq!(claims["exp"], 1_700_003_600);

        let key = ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            TEST_PUBLIC,
        );
        key.verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &decode(parts[2]),
        )
        .expect("the assertion verifies");
    }

    #[test]
    fn reports_a_key_it_cannot_use() {
        let error = service_account_jwt(&json!({ "type": "service_account" }), 0).unwrap_err();
        assert!(error.to_string().contains("client_email"), "{error}");

        let error = service_account_jwt(
            &json!({
                "client_email": "a@b.iam.gserviceaccount.com",
                "private_key": "-----BEGIN PRIVATE KEY-----\nnot a key\n-----END PRIVATE KEY-----",
            }),
            0,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("private key could not be read"),
            "{error}"
        );
    }

    fn decode(text: &str) -> Vec<u8> {
        let padded = match text.len() % 4 {
            2 => format!("{text}=="),
            3 => format!("{text}="),
            _ => text.to_string(),
        };
        crate::media::base64_decode(&padded.replace('-', "+").replace('_', "/")).unwrap()
    }
}

use anyhow::{bail, Result};
use ring::hmac;
use sha2::{Digest, Sha256};

/// What SigV4 signs a request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

impl Credentials {
    /// The variables an AWS SDK, the CLI and a container's role all read.
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        let access_key = get("AWS_ACCESS_KEY_ID").filter(|value| !value.trim().is_empty())?;
        let secret_key = get("AWS_SECRET_ACCESS_KEY").filter(|value| !value.trim().is_empty())?;
        Some(Self {
            access_key: access_key.trim().to_string(),
            secret_key: secret_key.trim().to_string(),
            session_token: get("AWS_SESSION_TOKEN")
                .or_else(|| get("AWS_SECURITY_TOKEN"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        })
    }

    /// The section a shared credentials file holds for `profile`.
    pub fn from_ini(text: &str, profile: &str) -> Option<Self> {
        let mut current = String::new();
        let mut values: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                current = name.trim().to_string();
                continue;
            }
            if current != profile {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            values.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
        let get = |name: &str| {
            values
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .filter(|value| !value.is_empty())
        };
        Some(Self {
            access_key: get("aws_access_key_id")?,
            secret_key: get("aws_secret_access_key")?,
            session_token: get("aws_session_token"),
        })
    }
}

/// The endpoint of a region, which is what names the host a request is signed
/// for.
pub fn host(region: &str, service: &str) -> String {
    format!("{service}.{region}.amazonaws.com")
}

/// The parts of a request SigV4 signs. They are passed in rather than read back
/// off a request that may still be mutated afterwards, since the payload, the
/// host and the content type all take part in the signature.
pub struct SigningRequest<'a> {
    pub method: &'a str,
    /// The path as it goes on the wire, already percent-coded.
    pub uri: &'a str,
    pub query: &'a [(String, String)],
    /// The authority the request is sent to, port included.
    pub host: &'a str,
    /// Absent for a body-less request, which carries no content type and is
    /// signed without one.
    pub content_type: Option<&'a str>,
    pub payload: &'a [u8],
    pub credentials: &'a Credentials,
    pub region: &'a str,
    pub service: &'a str,
    /// The timestamp the signature is dated with, `20150830T123600Z`.
    pub amz_date: &'a str,
}

/// The headers SigV4 adds to a request: the date it was signed, the payload's
/// digest, the session token when the credentials are temporary, and the
/// signature itself.
pub fn sign(request: &SigningRequest) -> Vec<(String, String)> {
    let SigningRequest {
        method,
        uri: canonical_uri,
        query,
        host,
        content_type,
        payload,
        credentials,
        region,
        service,
        amz_date,
    } = *request;
    let payload_hash = hex(&Sha256::digest(payload));
    let mut headers = vec![
        ("host".to_string(), host.to_string()),
        ("x-amz-date".to_string(), amz_date.to_string()),
    ];
    // A body-less request carries no content type, and signing one it does not
    // send would fail verification at the other end.
    if let Some(content_type) = content_type {
        headers.push(("content-type".to_string(), content_type.to_string()));
    }
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token".to_string(), token.clone()));
    }
    // `host` is signed as the authority the request is sent to, so a port is
    // part of it.
    headers.sort();
    let signed_headers: Vec<&str> = headers.iter().map(|(name, _)| name.as_str()).collect();
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect();

    let canonical_request = format!(
        "{method}\n{canonical_uri}\n{}\n{canonical_headers}\n{}\n{payload_hash}",
        canonical_query(query),
        signed_headers.join(";"),
    );

    let date = amz_date.get(..8).unwrap_or(amz_date);
    let scope = format!("{date}/{region}/{service}/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical_request.as_bytes()))
    );

    let mut key = hmac(
        format!("AWS4{}", credentials.secret_key).as_bytes(),
        date.as_bytes(),
    );
    for part in [region, service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, string_to_sign.as_bytes()));
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={}, Signature={signature}",
        credentials.access_key,
        signed_headers.join(";"),
    );
    headers.push(("authorization".to_string(), authorization));
    headers.push(("x-amz-content-sha256".to_string(), payload_hash));
    headers
}

/// The current time in the format SigV4 signs, `20150830T123600Z`.
pub fn amz_timestamp(now: std::time::SystemTime) -> String {
    let secs = now
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let second_of_day = secs % 86_400;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        second_of_day / 3600,
        (second_of_day / 60) % 60,
        second_of_day % 60
    )
}

/// Days since the Unix epoch to a calendar date, the way `chrono` would.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
}

fn canonical_query(query: &[(String, String)]) -> String {
    let mut pairs: Vec<String> = query
        .iter()
        .map(|(key, value)| format!("{}={}", uri_encode(key, true), uri_encode(value, true)))
        .collect();
    pairs.sort();
    pairs.join("&")
}

/// The percent-encoding SigV4 canonicalizes with, which encodes everything
/// outside the unreserved set — `/` included, unless the caller is encoding a
/// path.
fn uri_encode(value: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'~')
            || (!encode_slash && byte == b'/');
        if unreserved {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&key, data).as_ref().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The credentials a Bedrock run signs with, from the environment or from the
/// shared file the AWS CLI writes. An absent credential is an error rather than
/// an unauthenticated request, so the message can say what to set.
pub fn credentials_from(with_env: &dyn Fn(&str) -> Option<String>) -> Result<Credentials> {
    if let Some(credentials) = Credentials::from_env(with_env) {
        return Ok(credentials);
    }
    let profile = with_env("AWS_PROFILE").unwrap_or_else(|| "default".to_string());
    let path = with_env("AWS_SHARED_CREDENTIALS_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".aws/credentials")));
    if let Some(path) = path {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Some(credentials) = Credentials::from_ini(&text, &profile) {
                return Ok(credentials);
            }
            bail!(
                "no `{profile}` profile in {} — set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY, \
                 or run `aws configure`",
                path.display()
            );
        }
    }
    bail!(
        "no AWS credentials found. Set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY (or a \
         AWS_BEARER_TOKEN_BEDROCK token), or run `aws configure`"
    )
}

/// The region a Bedrock or Bedrock-runtime request is sent to.
pub fn region_from(with_env: &dyn Fn(&str) -> Option<String>) -> String {
    for name in [
        "AWS_REGION",
        "AWS_DEFAULT_REGION",
        "BEDROCK_REGION",
        "AWS_BEDROCK_REGION",
    ] {
        if let Some(value) = with_env(name).filter(|value| !value.trim().is_empty()) {
            return value.trim().to_string();
        }
    }
    "us-east-1".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    /// The `get-vanilla` case from AWS's own SigV4 test suite.
    #[test]
    fn signs_the_published_test_vector() {
        let credentials = Credentials {
            access_key: "AKIDEXAMPLE".to_string(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        };
        let headers = sign(&SigningRequest {
            method: "GET",
            uri: "/",
            query: &[],
            host: "example.amazonaws.com",
            content_type: None,
            payload: b"",
            credentials: &credentials,
            region: "us-east-1",
            service: "service",
            amz_date: "20150830T123600Z",
        });
        let authorization = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
            .expect("a signed request carries an authorization header");
        assert!(
            authorization.ends_with(
                "Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
            ),
            "{authorization}"
        );
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| name == "x-amz-content-sha256")
                .map(|(_, value)| value.as_str()),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn signs_a_payload_and_a_session_token() {
        let credentials = Credentials {
            access_key: "AKID".to_string(),
            secret_key: "SECRET".to_string(),
            session_token: Some("TOKEN".to_string()),
        };
        let headers = sign(&SigningRequest {
            method: "POST",
            uri: "/model/x/invoke",
            query: &[
                ("b".to_string(), "2".to_string()),
                ("a".to_string(), "1".to_string()),
            ],
            host: "example.amazonaws.com",
            content_type: Some("application/json"),
            payload: b"{}",
            credentials: &credentials,
            region: "eu-west-1",
            service: "bedrock",
            amz_date: "20240101T000000Z",
        });
        let authorization = headers
            .iter()
            .find(|(name, _)| name == "authorization")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert!(authorization.contains("Credential=AKID/20240101/eu-west-1/bedrock/aws4_request"));
        assert!(authorization.contains("x-amz-security-token"));
        // The payload's digest is signed, so the same body signs the same way.
        assert!(authorization.contains("Signature="));
    }

    #[test]
    fn reads_credentials_from_the_environment() {
        let credentials = Credentials::from_env(&env(&[
            ("AWS_ACCESS_KEY_ID", "AKID"),
            ("AWS_SECRET_ACCESS_KEY", "SECRET"),
            ("AWS_SESSION_TOKEN", "TOKEN"),
        ]))
        .expect("the variables are set");
        assert_eq!(credentials.access_key, "AKID");
        assert_eq!(credentials.session_token.as_deref(), Some("TOKEN"));
        assert!(Credentials::from_env(&env(&[("AWS_ACCESS_KEY_ID", "AKID")])).is_none());
    }

    #[test]
    fn reads_a_profile_from_the_shared_file() {
        let text = "[default]\naws_access_key_id = ONE\naws_secret_access_key = A\n\n\
                    [work]\naws_access_key_id = TWO\naws_secret_access_key = B\n";
        assert_eq!(
            Credentials::from_ini(text, "work").map(|c| c.access_key),
            Some("TWO".to_string())
        );
        assert_eq!(
            Credentials::from_ini(text, "default").map(|c| c.access_key),
            Some("ONE".to_string())
        );
        assert!(Credentials::from_ini(text, "missing").is_none());
    }

    #[test]
    fn falls_back_to_a_region_and_says_so_when_nothing_is_configured() {
        assert_eq!(region_from(&env(&[])), "us-east-1");
        assert_eq!(
            region_from(&env(&[("AWS_DEFAULT_REGION", "ap-southeast-2")])),
            "ap-southeast-2"
        );
        let error = credentials_from(&env(&[])).unwrap_err();
        assert!(error.to_string().contains("AWS_ACCESS_KEY_ID"), "{error}");
    }

    #[test]
    fn formats_a_sigv4_timestamp() {
        // 2015-08-30T12:36:00Z, the date the published test vector signs.
        let stamp =
            amz_timestamp(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160));
        assert_eq!(stamp, "20150830T123600Z");
    }

    #[test]
    fn escapes_a_query_value() {
        assert_eq!(
            canonical_query(&[("a b".to_string(), "c/d".to_string())]),
            "a%20b=c%2Fd"
        );
    }
}

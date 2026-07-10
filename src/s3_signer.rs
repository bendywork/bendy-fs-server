use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use web_time::SystemTime;

use crate::types::BackendConfig;

type HmacSha256 = Hmac<Sha256>;

const LONG_DATE_FORMAT: &str = "%Y%m%dT%H%M%SZ";
const SHORT_DATE_FORMAT: &str = "%Y%m%d";

pub fn now_utc() -> (String, String) {
    let now = SystemTime::now();
    let dur = now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = dur.as_secs();

    // Manual UTC formatting to avoid chrono dependency
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;

    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Calculate year/month/day from days_since_epoch (UNIX epoch = 1970-01-01)
    let mut year: i64 = 1970;
    let mut remaining_days = days_since_epoch as i64;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        year += 1;
    }

    let month_days = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month: u32 = 1;
    for &md in &month_days {
        if remaining_days < md as i64 {
            break;
        }
        remaining_days -= md as i64;
        month += 1;
    }

    let day = remaining_days as u32 + 1;

    let long_date = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        year, month, day, hours, minutes, seconds
    );
    let short_date = format!("{:04}{:02}{:02}", year, month, day);

    (long_date, short_date)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn build_canonical_uri(key: &str) -> String {
    let encoded = key
        .split('/')
        .map(|seg| {
            url_encode_except_slash(seg)
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("/{}", encoded)
}

fn url_encode_except_slash(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b'/' => {
                result.push('/');
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

fn url_encode_query(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b'=' => result.push_str("%3D"),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{}", secret).as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// Build a presigned URL for PUT (upload) operation
pub fn presign_put(
    config: &BackendConfig,
    key: &str,
    content_type: &str,
    expires_seconds: u64,
) -> String {
    presign_url(config, key, "PUT", Some(content_type), expires_seconds, None)
}

/// Build a presigned URL for GET (download) operation
pub fn presign_get(
    config: &BackendConfig,
    key: &str,
    expires_seconds: u64,
    filename: Option<&str>,
) -> String {
    presign_url(config, key, "GET", None, expires_seconds, filename)
}

fn presign_url(
    config: &BackendConfig,
    key: &str,
    method: &str,
    content_type: Option<&str>,
    expires_seconds: u64,
    filename: Option<&str>,
) -> String {
    let (long_date, short_date) = now_utc();
    let region = if config.region.is_empty() { "us-east-1" } else { &config.region };
    let service = "s3";
    let algorithm = "AWS4-HMAC-SHA256";
    let credential = format!(
        "{}/{}/{}/{}/aws4_request",
        config.access_key_id, short_date, region, service
    );

    let canonical_uri = build_canonical_uri(key);

    // Build query parameters (sorted alphabetically)
    let mut query_pairs: Vec<(String, String)> = Vec::new();

    query_pairs.push(("X-Amz-Algorithm".into(), algorithm.into()));
    query_pairs.push(("X-Amz-Credential".into(), credential.clone()));
    query_pairs.push(("X-Amz-Date".into(), long_date.clone()));
    query_pairs.push(("X-Amz-Expires".into(), expires_seconds.to_string()));
    query_pairs.push((
        "X-Amz-SignedHeaders".into(),
        "host".into(),
    ));

    if let Some(ct) = content_type {
        query_pairs.push(("Content-Type".into(), ct.into()));
    }

    if let Some(fname) = filename {
        query_pairs.push((
            "response-content-disposition".into(),
            format!(
                "attachment; filename*=UTF-8''{}",
                url_encode_rfc5987(fname)
            ),
        ));
    }

    // Sort by key, then by value for determinism
    query_pairs.sort_by(|a, b| {
        let key_cmp = a.0.as_bytes().cmp(b.0.as_bytes());
        if key_cmp == std::cmp::Ordering::Equal {
            a.1.as_bytes().cmp(b.1.as_bytes())
        } else {
            key_cmp
        }
    });

    let canonical_query_string = query_pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode_query(k), url_encode_query(v)))
        .collect::<Vec<_>>()
        .join("&");

    // Canonical headers (must include host and any x-amz-* headers in signed headers)
    let host = extract_host(&config.endpoint);
    let canonical_headers = format!("host:{}\n", host);
    let signed_headers = "host";

    // Payload hash is always UNSIGNED-PAYLOAD for presigned URLs
    let payload_hash = "UNSIGNED-PAYLOAD";

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        canonical_query_string,
        canonical_headers,
        signed_headers,
        payload_hash
    );

    let string_to_sign = format!(
        "{}\n{}\n{}/{}/{}/aws4_request\n{}",
        algorithm,
        long_date,
        short_date,
        region,
        service,
        sha256_hex(canonical_request.as_bytes())
    );

    let signing_key = derive_signing_key(
        &config.secret_access_key,
        &short_date,
        region,
        service,
    );

    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    // Build final URL
    let base = config.endpoint.trim_end_matches('/');
    if config.force_path_style {
        format!(
            "{}/{}/{}?{}&X-Amz-Signature={}",
            base,
            config.bucket,
            key.trim_start_matches('/'),
            canonical_query_string,
            signature
        )
    } else {
        format!(
            "{}/{}/{}?{}&X-Amz-Signature={}",
            base,
            config.bucket,
            key.trim_start_matches('/'),
            canonical_query_string,
            signature
        )
    }
}

pub fn extract_host(endpoint: &str) -> String {
    endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_string()
}

fn url_encode_rfc5987(value: &str) -> String {
    let mut result = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'!' | b'#' | b'$' | b'&'
            | b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Build the Authorization header for a direct S3 request (non-presigned)
pub fn build_auth_header(
    config: &BackendConfig,
    method: &str,
    key: &str,
    content_type: Option<&str>,
    body: &[u8],
) -> String {
    let (long_date, short_date) = now_utc();
    let region = if config.region.is_empty() { "us-east-1" } else { &config.region };
    let service = "s3";
    let algorithm = "AWS4-HMAC-SHA256";
    let credential = format!(
        "{}/{}/{}/{}/aws4_request",
        config.access_key_id, short_date, region, service
    );

    let canonical_uri = build_canonical_uri(key);
    let canonical_query_string = "";
    let host = extract_host(&config.endpoint);

    let payload_hash = sha256_hex(body);

    let content_type_str = content_type.unwrap_or("application/octet-stream");
    let canonical_headers = format!(
        "content-type:{}\nhost:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
        content_type_str, host, payload_hash, long_date
    );
    let signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date";

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        method,
        canonical_uri,
        canonical_query_string,
        canonical_headers,
        signed_headers,
        payload_hash
    );

    let string_to_sign = format!(
        "{}\n{}\n{}/{}/{}/aws4_request\n{}",
        algorithm,
        long_date,
        short_date,
        region,
        service,
        sha256_hex(canonical_request.as_bytes())
    );

    let signing_key = derive_signing_key(
        &config.secret_access_key,
        &short_date,
        region,
        service,
    );

    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));

    format!(
        "{} Credential={},SignedHeaders={},Signature={}",
        algorithm, credential, signed_headers, signature
    )
}

/// Build the S3 request URL (path-style or virtual-hosted)
pub fn build_object_url(config: &BackendConfig, key: &str) -> String {
    let base = config.endpoint.trim_end_matches('/');
    format!("{}/{}/{}", base, config.bucket, key.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_host() {
        assert_eq!(extract_host("https://s3.amazonaws.com"), "s3.amazonaws.com");
        assert_eq!(extract_host("http://localhost:9000"), "localhost");
        assert_eq!(extract_host("https://s3.hi168.com/path"), "s3.hi168.com");
    }

    #[test]
    fn test_sha256() {
        let hash = sha256_hex(b"hello");
        assert_eq!(hash.len(), 64);
    }
}

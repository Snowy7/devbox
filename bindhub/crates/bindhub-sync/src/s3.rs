use crate::{ObjectKey, ObjectMetadata, PutOutcome, RemoteBlobProvider, SyncError, SyncResult};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fmt;
use std::io::Read;
use time::format_description::FormatItem;
use time::macros::format_description;
use time::OffsetDateTime;
use ureq::{Agent, Error as UreqError};
use url::Url;

type HmacSha256 = Hmac<Sha256>;

const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'=')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');
const AMZ_DATE_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");
const SHORT_DATE_FORMAT: &[FormatItem<'_>] = format_description!("[year][month][day]");

#[derive(Clone, PartialEq, Eq)]
pub struct S3CompatibleConfig {
    endpoint: Url,
    bucket: String,
    region: String,
    prefix: Option<String>,
    credentials: S3CredentialsSource,
}

impl S3CompatibleConfig {
    pub fn new(
        endpoint: impl AsRef<str>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        prefix: Option<impl Into<String>>,
        credentials: S3CredentialsSource,
    ) -> SyncResult<Self> {
        let endpoint = parse_endpoint(endpoint.as_ref())?;
        let bucket = validate_bucket(bucket.into())?;
        let region = validate_region(region.into())?;
        let prefix = prefix.map(Into::into).map(normalize_prefix).transpose()?;

        Ok(Self {
            endpoint,
            bucket,
            region,
            prefix,
            credentials,
        })
    }

    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn prefix(&self) -> Option<&str> {
        self.prefix.as_deref()
    }

    pub fn credentials_source(&self) -> &S3CredentialsSource {
        &self.credentials
    }

    pub fn redacted(&self) -> S3RedactedConfig {
        S3RedactedConfig {
            endpoint_host: self.endpoint_host(),
            bucket: self.bucket.clone(),
            region: self.region.clone(),
            prefix: self.prefix.clone(),
            access_key_env: self.credentials.access_key_env().to_string(),
            secret_key_env: self.credentials.secret_key_env().to_string(),
            session_token_env: self.credentials.session_token_env().map(str::to_string),
        }
    }

    pub fn endpoint_host(&self) -> String {
        self.endpoint
            .host_str()
            .map(str::to_string)
            .unwrap_or_else(|| "-".to_string())
    }

    fn effective_key(&self, key: &ObjectKey) -> String {
        match &self.prefix {
            Some(prefix) => format!("{prefix}/{}", key.as_str()),
            None => key.as_str().to_string(),
        }
    }

    fn object_url(&self, key: &ObjectKey) -> String {
        let mut base = self.endpoint.as_str().trim_end_matches('/').to_string();
        base.push('/');
        base.push_str(&encode_path_segment(&self.bucket));
        for segment in self.effective_key(key).split('/') {
            base.push('/');
            base.push_str(&encode_path_segment(segment));
        }
        base
    }

    fn list_url(&self, prefix: Option<&ObjectKey>) -> String {
        let mut url = self.endpoint.clone();
        url.set_path(&format!("/{}", encode_path_segment(&self.bucket)));
        url.set_query(None);
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("list-type", "2");
            if let Some(prefix) = self.effective_list_prefix(prefix) {
                query.append_pair("prefix", &prefix);
            }
        }
        url.to_string()
    }

    fn effective_list_prefix(&self, prefix: Option<&ObjectKey>) -> Option<String> {
        match (&self.prefix, prefix) {
            (Some(config_prefix), Some(prefix)) => {
                Some(format!("{config_prefix}/{}", prefix.as_str()))
            }
            (Some(config_prefix), None) => Some(format!("{config_prefix}/")),
            (None, Some(prefix)) => Some(prefix.as_str().to_string()),
            (None, None) => None,
        }
    }

    fn strip_config_prefix(&self, key: &str) -> Option<String> {
        if let Some(prefix) = &self.prefix {
            let scoped_prefix = format!("{prefix}/");
            return key
                .strip_prefix(&scoped_prefix)
                .map(str::to_string)
                .filter(|relative| !relative.is_empty());
        }
        Some(key.to_string())
    }
}

impl fmt::Debug for S3CompatibleConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.redacted().fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3RedactedConfig {
    pub endpoint_host: String,
    pub bucket: String,
    pub region: String,
    pub prefix: Option<String>,
    pub access_key_env: String,
    pub secret_key_env: String,
    pub session_token_env: Option<String>,
}

impl fmt::Display for S3RedactedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "kind=s3 endpoint_host={} bucket={} region={} prefix={} access_key_env={} secret_key_env={} session_token_env={}",
            self.endpoint_host,
            self.bucket,
            self.region,
            self.prefix.as_deref().unwrap_or("-"),
            self.access_key_env,
            self.secret_key_env,
            self.session_token_env.as_deref().unwrap_or("-")
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum S3CredentialsSource {
    Env {
        access_key_env: String,
        secret_key_env: String,
        session_token_env: Option<String>,
    },
    #[default]
    StandardEnv,
}

impl S3CredentialsSource {
    pub fn env(
        access_key_env: impl Into<String>,
        secret_key_env: impl Into<String>,
        session_token_env: Option<impl Into<String>>,
    ) -> SyncResult<Self> {
        let access_key_env = validate_env_name(access_key_env.into(), "access key env")?;
        let secret_key_env = validate_env_name(secret_key_env.into(), "secret key env")?;
        let session_token_env = session_token_env
            .map(Into::into)
            .map(|name| validate_env_name(name, "session token env"))
            .transpose()?;
        Ok(Self::Env {
            access_key_env,
            secret_key_env,
            session_token_env,
        })
    }

    pub fn access_key_env(&self) -> &str {
        match self {
            Self::Env { access_key_env, .. } => access_key_env,
            Self::StandardEnv => "AWS_ACCESS_KEY_ID",
        }
    }

    pub fn secret_key_env(&self) -> &str {
        match self {
            Self::Env { secret_key_env, .. } => secret_key_env,
            Self::StandardEnv => "AWS_SECRET_ACCESS_KEY",
        }
    }

    pub fn session_token_env(&self) -> Option<&str> {
        match self {
            Self::Env {
                session_token_env, ..
            } => session_token_env.as_deref(),
            Self::StandardEnv => Some("AWS_SESSION_TOKEN"),
        }
    }

    fn load(&self) -> SyncResult<S3Credentials> {
        let access_key = load_required_env(self.access_key_env(), "access key")?;
        let secret_key = load_required_env(self.secret_key_env(), "secret key")?;
        let session_token = self
            .session_token_env()
            .and_then(|name| env::var(name).ok())
            .filter(|value| !value.is_empty());
        S3Credentials::new(access_key, secret_key, session_token)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct S3Credentials {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
}

impl S3Credentials {
    pub fn new(
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        session_token: Option<impl Into<String>>,
    ) -> SyncResult<Self> {
        let access_key = access_key.into();
        let secret_key = secret_key.into();
        if access_key.trim().is_empty() {
            return Err(SyncError::RemoteCredentials(
                "access key cannot be empty".to_string(),
            ));
        }
        if secret_key.trim().is_empty() {
            return Err(SyncError::RemoteCredentials(
                "secret key cannot be empty".to_string(),
            ));
        }
        let session_token = session_token.map(Into::into);
        if session_token
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(SyncError::RemoteCredentials(
                "session token cannot be empty when provided".to_string(),
            ));
        }
        Ok(Self {
            access_key,
            secret_key,
            session_token,
        })
    }
}

impl fmt::Debug for S3Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Credentials")
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct S3CompatibleBlobProvider {
    config: S3CompatibleConfig,
    credentials: S3Credentials,
    agent: Agent,
}

impl S3CompatibleBlobProvider {
    pub fn from_env(config: S3CompatibleConfig) -> SyncResult<Self> {
        let credentials = config.credentials_source().load()?;
        Ok(Self::new(config, credentials))
    }

    pub fn new(config: S3CompatibleConfig, credentials: S3Credentials) -> Self {
        Self {
            config,
            credentials,
            agent: Agent::new(),
        }
    }

    pub fn config(&self) -> &S3CompatibleConfig {
        &self.config
    }

    fn send(&self, method: &str, key: &ObjectKey, body: Option<&[u8]>) -> SyncResult<S3Response> {
        self.send_with_headers(method, key, body, &[])
    }

    fn send_url(&self, method: &str, url: &str, body: Option<&[u8]>) -> SyncResult<S3Response> {
        self.send_url_with_headers(method, url, body, &[])
    }

    fn send_with_headers(
        &self,
        method: &str,
        key: &ObjectKey,
        body: Option<&[u8]>,
        extra_headers: &[(&str, &str)],
    ) -> SyncResult<S3Response> {
        let url = self.config.object_url(key);
        self.send_url_with_headers(method, &url, body, extra_headers)
    }

    fn send_url_with_headers(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
        extra_headers: &[(&str, &str)],
    ) -> SyncResult<S3Response> {
        let body = body.unwrap_or_default();
        let payload_hash = sha256_hex(body);
        let signed = sign_request(
            method,
            &url,
            &payload_hash,
            &self.config.region,
            &self.credentials,
            extra_headers,
            OffsetDateTime::now_utc(),
        )?;

        let mut request = self
            .agent
            .request(method, &url)
            .set("Authorization", &signed.authorization)
            .set("Host", &signed.host)
            .set("x-amz-content-sha256", &payload_hash)
            .set("x-amz-date", &signed.amz_date);
        if let Some(token) = &self.credentials.session_token {
            request = request.set("x-amz-security-token", token);
        }
        for (name, value) in extra_headers {
            request = request.set(name, value);
        }

        let response = match method {
            "PUT" => request.send_bytes(body),
            _ => request.call(),
        };

        match response {
            Ok(response) => {
                let status = response.status();
                let content_length = response
                    .header("content-length")
                    .and_then(|value| value.parse::<u64>().ok());
                let bytes = if method == "HEAD" {
                    Vec::new()
                } else {
                    read_response_bytes(response)?
                };
                Ok(S3Response {
                    status,
                    bytes,
                    content_length,
                })
            }
            Err(UreqError::Status(status, response)) => {
                let content_length = response
                    .header("content-length")
                    .and_then(|value| value.parse::<u64>().ok());
                let bytes = if method == "HEAD" {
                    Vec::new()
                } else {
                    read_response_bytes(response)?
                };
                Ok(S3Response {
                    status,
                    bytes,
                    content_length,
                })
            }
            Err(UreqError::Transport(error)) => {
                Err(SyncError::RemoteTransport(redact_transport_error(error)))
            }
        }
    }
}

fn read_response_bytes(response: ureq::Response) -> SyncResult<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
    Ok(bytes)
}

impl RemoteBlobProvider for S3CompatibleBlobProvider {
    fn put(&self, key: &ObjectKey, bytes: &[u8]) -> SyncResult<PutOutcome> {
        let response =
            self.send_with_headers("PUT", key, Some(bytes), &[("If-None-Match", "*")])?;
        if (200..300).contains(&response.status) {
            return Ok(PutOutcome {
                uploaded: true,
                size_bytes: bytes.len() as u64,
            });
        }
        if matches!(response.status, 409 | 412) {
            if let Some(existing) = self.get(key)? {
                if existing == bytes {
                    return Ok(PutOutcome {
                        uploaded: false,
                        size_bytes: existing.len() as u64,
                    });
                }
            }
            return Err(SyncError::RemoteObjectAlreadyExists { key: key.clone() });
        }

        Err(SyncError::RemoteTransport(format!(
            "S3 PUT failed with HTTP status {}",
            response.status
        )))
    }

    fn get(&self, key: &ObjectKey) -> SyncResult<Option<Vec<u8>>> {
        let response = self.send("GET", key, None)?;
        match response.status {
            200 => Ok(Some(response.bytes)),
            404 => Ok(None),
            status => Err(SyncError::RemoteTransport(format!(
                "S3 GET failed with HTTP status {status}"
            ))),
        }
    }

    fn head(&self, key: &ObjectKey) -> SyncResult<Option<ObjectMetadata>> {
        let response = self.send("HEAD", key, None)?;
        match response.status {
            200 => Ok(Some(ObjectMetadata {
                key: key.clone(),
                size_bytes: response.content_length.unwrap_or(0),
            })),
            404 => Ok(None),
            status => Err(SyncError::RemoteTransport(format!(
                "S3 HEAD failed with HTTP status {status}"
            ))),
        }
    }

    fn list(&self, prefix: Option<&ObjectKey>) -> SyncResult<Vec<ObjectMetadata>> {
        let url = self.config.list_url(prefix);
        let response = self.send_url("GET", &url, None)?;
        match response.status {
            200 => {
                let body = String::from_utf8(response.bytes)
                    .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
                let parsed: S3ListBucketResult = quick_xml::de::from_str(&body)
                    .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
                parsed
                    .contents
                    .into_iter()
                    .filter_map(|object| {
                        self.config
                            .strip_config_prefix(&object.key)
                            .map(|relative| {
                                Ok(ObjectMetadata {
                                    key: ObjectKey::new(relative)?,
                                    size_bytes: object.size,
                                })
                            })
                    })
                    .collect()
            }
            status => Err(SyncError::RemoteTransport(format!(
                "S3 LIST failed with HTTP status {status}"
            ))),
        }
    }
}

#[derive(Debug)]
struct S3Response {
    status: u16,
    bytes: Vec<u8>,
    content_length: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct S3ListBucketResult {
    #[serde(rename = "Contents", default)]
    contents: Vec<S3ListObject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct S3ListObject {
    key: String,
    size: u64,
}

#[derive(Debug)]
struct SignedRequest {
    authorization: String,
    amz_date: String,
    host: String,
}

fn sign_request(
    method: &str,
    url: &str,
    payload_hash: &str,
    region: &str,
    credentials: &S3Credentials,
    extra_headers: &[(&str, &str)],
    now: OffsetDateTime,
) -> SyncResult<SignedRequest> {
    let url = Url::parse(url).map_err(|error| SyncError::RemoteConfig(error.to_string()))?;
    let host = host_header(&url)?;
    let amz_date = now
        .format(AMZ_DATE_FORMAT)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
    let short_date = now
        .format(SHORT_DATE_FORMAT)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;

    let mut headers = vec![
        ("host".to_string(), host.clone()),
        ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
        ("x-amz-date".to_string(), amz_date.clone()),
    ];
    if let Some(token) = &credentials.session_token {
        headers.push(("x-amz-security-token".to_string(), token.to_string()));
    }
    for (name, value) in extra_headers {
        let name = canonical_header_name(name)?;
        if headers.iter().any(|(existing, _)| existing == &name) {
            return Err(SyncError::RemoteConfig(format!(
                "duplicate signed header {name}"
            )));
        }
        headers.push((name, canonical_header_value(value)));
    }
    headers.sort_by(|left, right| left.0.cmp(&right.0));
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect::<String>();
    let signed_headers = headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical_request = format!(
        "{method}\n{}\n{}\n{canonical_headers}\n{signed_headers}\n{payload_hash}",
        url.path(),
        url.query().unwrap_or_default()
    );
    let scope = format!("{short_date}/{region}/s3/aws4_request");
    let canonical_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{canonical_hash}");
    let signing_key = signing_key(&credentials.secret_key, &short_date, region)?;
    let signature = hex_encode(&hmac_bytes(&signing_key, string_to_sign.as_bytes())?);
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key
    );

    Ok(SignedRequest {
        authorization,
        amz_date,
        host,
    })
}

fn signing_key(secret_key: &str, date: &str, region: &str) -> SyncResult<Vec<u8>> {
    let k_date = hmac_bytes(format!("AWS4{secret_key}").as_bytes(), date.as_bytes())?;
    let k_region = hmac_bytes(&k_date, region.as_bytes())?;
    let k_service = hmac_bytes(&k_region, b"s3")?;
    hmac_bytes(&k_service, b"aws4_request")
}

fn hmac_bytes(key: &[u8], data: &[u8]) -> SyncResult<Vec<u8>> {
    let mut mac = HmacSha256::new_from_slice(key)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn parse_endpoint(value: &str) -> SyncResult<Url> {
    let url = Url::parse(value)
        .map_err(|error| SyncError::RemoteConfig(format!("endpoint URL is invalid: {error}")))?;
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(SyncError::RemoteConfig(format!(
                "endpoint URL scheme must be http or https, got {scheme}"
            )));
        }
    }
    if url.host_str().is_none() {
        return Err(SyncError::RemoteConfig(
            "endpoint URL must include a host".to_string(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SyncError::RemoteConfig(
            "endpoint URL must not include username or password".to_string(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(SyncError::RemoteConfig(
            "endpoint URL must not include query or fragment".to_string(),
        ));
    }
    Ok(url)
}

fn validate_bucket(value: String) -> SyncResult<String> {
    if value.trim().is_empty() {
        return Err(SyncError::RemoteConfig(
            "bucket cannot be empty".to_string(),
        ));
    }
    if value.contains('/') || value.contains('\\') {
        return Err(SyncError::RemoteConfig(
            "bucket must not contain path separators".to_string(),
        ));
    }
    Ok(value)
}

fn validate_region(value: String) -> SyncResult<String> {
    if value.trim().is_empty() {
        return Err(SyncError::RemoteConfig(
            "region cannot be empty".to_string(),
        ));
    }
    Ok(value)
}

fn normalize_prefix(value: String) -> SyncResult<String> {
    let trimmed = value.trim_matches('/').to_string();
    if trimmed.is_empty() {
        return Err(SyncError::RemoteConfig(
            "prefix cannot be empty when provided".to_string(),
        ));
    }
    ObjectKey::new(trimmed.clone())?;
    Ok(trimmed)
}

fn validate_env_name(value: String, label: &str) -> SyncResult<String> {
    if value.is_empty() {
        return Err(SyncError::RemoteConfig(format!("{label} cannot be empty")));
    }
    if !value
        .bytes()
        .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err(SyncError::RemoteConfig(format!(
            "{label} must contain only ASCII letters, digits, or underscore"
        )));
    }
    Ok(value)
}

fn canonical_header_name(name: &str) -> SyncResult<String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty()
        || name
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-'))
    {
        return Err(SyncError::RemoteConfig(
            "signed header names must contain only ASCII letters, digits, or '-'".to_string(),
        ));
    }
    Ok(name)
}

fn canonical_header_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn load_required_env(name: &str, label: &str) -> SyncResult<String> {
    env::var(name).map_err(|_| {
        SyncError::RemoteCredentials(format!("missing {label}; set environment variable {name}"))
    })
}

fn encode_path_segment(segment: &str) -> String {
    utf8_percent_encode(segment, PATH_ENCODE_SET).to_string()
}

fn host_header(url: &Url) -> SyncResult<String> {
    let host = url
        .host_str()
        .ok_or_else(|| SyncError::RemoteConfig("endpoint URL must include a host".to_string()))?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

fn redact_transport_error(error: ureq::Transport) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn s3_config_redacts_credential_values_and_reports_env_names() {
        let config = S3CompatibleConfig::new(
            "https://account.r2.cloudflarestorage.com",
            "bindhub-alpha",
            "auto",
            Some("accounts/acct/projects"),
            S3CredentialsSource::env(
                "BINDHUB_R2_ACCESS_KEY_ID",
                "BINDHUB_R2_SECRET_ACCESS_KEY",
                Some("BINDHUB_R2_SESSION_TOKEN"),
            )
            .expect("credential source parses"),
        )
        .expect("config parses");

        let redacted = config.redacted().to_string();

        assert!(redacted.contains("endpoint_host=account.r2.cloudflarestorage.com"));
        assert!(redacted.contains("bucket=bindhub-alpha"));
        assert!(redacted.contains("region=auto"));
        assert!(redacted.contains("prefix=accounts/acct/projects"));
        assert!(redacted.contains("access_key_env=BINDHUB_R2_ACCESS_KEY_ID"));
        assert!(redacted.contains("secret_key_env=BINDHUB_R2_SECRET_ACCESS_KEY"));
        assert!(!redacted.contains("secret-value"));
    }

    #[test]
    fn s3_config_rejects_unsafe_prefixes_and_buckets() {
        let source = S3CredentialsSource::default();

        assert!(S3CompatibleConfig::new(
            "https://example.com",
            "bucket",
            "auto",
            Some("../escape"),
            source.clone(),
        )
        .is_err());
        assert!(S3CompatibleConfig::new(
            "https://example.com",
            "bucket/name",
            "auto",
            None::<String>,
            source.clone(),
        )
        .is_err());
        assert!(S3CompatibleConfig::new(
            "file:///tmp/bucket",
            "bucket",
            "auto",
            None::<String>,
            source.clone(),
        )
        .is_err());
        assert!(S3CompatibleConfig::new(
            "https://access:secret@example.com",
            "bucket",
            "auto",
            None::<String>,
            source,
        )
        .is_err());
    }

    #[test]
    fn s3_config_builds_path_style_urls_with_prefix() {
        let config = S3CompatibleConfig::new(
            "https://example.com/root/",
            "bucket",
            "us-east-1",
            Some("prefix"),
            S3CredentialsSource::default(),
        )
        .expect("config parses");
        let key = ObjectKey::new("encrypted/blobs/a b").expect("key parses");

        assert_eq!(
            config.object_url(&key),
            "https://example.com/root/bucket/prefix/encrypted/blobs/a%20b"
        );
    }

    #[test]
    fn s3_put_does_not_require_read_before_create() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::{Arc, Mutex};
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").expect("test server binds");
        let endpoint = format!("http://{}", listener.local_addr().expect("server addr"));
        let methods = Arc::new(Mutex::new(Vec::new()));
        let server_methods = Arc::clone(&methods);
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request accepted");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 1024];
            let header_end = loop {
                let read = stream.read(&mut buffer).expect("request reads");
                assert_ne!(read, 0, "request ended before headers");
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index;
                }
            };

            let headers = std::str::from_utf8(&bytes[..header_end]).expect("headers are utf8");
            let request_line = headers.lines().next().expect("request line exists");
            let method = request_line
                .split_whitespace()
                .next()
                .expect("method exists")
                .to_string();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                })
                .unwrap_or(0);
            let body_start = header_end + 4;
            while bytes.len() < body_start + content_length {
                let read = stream.read(&mut buffer).expect("body reads");
                assert_ne!(read, 0, "request ended before body");
                bytes.extend_from_slice(&buffer[..read]);
            }

            server_methods
                .lock()
                .expect("methods lock")
                .push(method.clone());
            if method == "GET" {
                stream
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                    )
                    .expect("403 writes");
            } else {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                    .expect("200 writes");
            }
        });

        let config = S3CompatibleConfig::new(
            endpoint,
            "bucket",
            "auto",
            None::<String>,
            S3CredentialsSource::default(),
        )
        .expect("config parses");
        let credentials =
            S3Credentials::new("access", "secret", None::<String>).expect("credentials parse");
        let provider = S3CompatibleBlobProvider::new(config, credentials);
        let key = ObjectKey::new("packs/revision.loompack").expect("key parses");

        let outcome = provider.put(&key, b"pack").expect("put succeeds");

        assert!(outcome.uploaded);
        assert_eq!(outcome.size_bytes, 4);
        server.join().expect("server exits");
        assert_eq!(methods.lock().expect("methods lock").as_slice(), ["PUT"]);
    }

    #[test]
    fn s3_credentials_debug_is_redacted() {
        let credentials = S3Credentials::new(
            "ACCESS_VALUE_SHOULD_NOT_APPEAR",
            "SECRET_VALUE_SHOULD_NOT_APPEAR",
            Some("TOKEN_VALUE_SHOULD_NOT_APPEAR"),
        )
        .expect("credentials parse");
        let debug = format!("{credentials:?}");

        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("ACCESS_VALUE_SHOULD_NOT_APPEAR"));
        assert!(!debug.contains("SECRET_VALUE_SHOULD_NOT_APPEAR"));
        assert!(!debug.contains("TOKEN_VALUE_SHOULD_NOT_APPEAR"));
    }

    #[test]
    fn s3_credentials_reject_empty_required_values_and_empty_session_token() {
        assert!(S3Credentials::new("", "secret", None::<String>).is_err());
        assert!(S3Credentials::new("access", " ", None::<String>).is_err());
        assert!(S3Credentials::new("access", "secret", Some("")).is_err());
    }

    #[test]
    fn missing_named_credentials_reports_env_names_not_values() {
        let source = S3CredentialsSource::env(
            "BINDHUB_TEST_MISSING_ACCESS_KEY",
            "BINDHUB_TEST_MISSING_SECRET_KEY",
            None::<String>,
        )
        .expect("source parses");

        let error = source.load().expect_err("missing env vars fail");
        let message = error.to_string();

        assert!(message.contains("BINDHUB_TEST_MISSING_ACCESS_KEY"));
        assert!(!message.contains("BINDHUB_TEST_MISSING_SECRET_KEY_VALUE"));
    }

    #[test]
    fn sigv4_authorization_never_contains_secret_key() {
        let credentials = S3Credentials::new(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            None::<String>,
        )
        .expect("credentials parse");
        let signed = sign_request(
            "GET",
            "https://example.com/bucket/key",
            EMPTY_SHA256,
            "us-east-1",
            &credentials,
            &[],
            OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("timestamp parses"),
        )
        .expect("request signs");

        assert!(signed.authorization.contains("Credential=AKIDEXAMPLE/"));
        assert!(!signed.authorization.contains("wJalrXUtnFEMI"));
    }

    #[test]
    fn sigv4_signs_conditional_put_headers() {
        let credentials =
            S3Credentials::new("AKIDEXAMPLE", "SECRET", None::<String>).expect("credentials parse");
        let signed = sign_request(
            "PUT",
            "https://example.com/bucket/key",
            EMPTY_SHA256,
            "auto",
            &credentials,
            &[("If-None-Match", "*")],
            OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("timestamp parses"),
        )
        .expect("request signs");

        assert!(signed
            .authorization
            .contains("SignedHeaders=host;if-none-match;x-amz-content-sha256;x-amz-date"));
        assert!(!signed.authorization.contains("SECRET"));
    }

    #[test]
    fn s3_list_response_strips_config_prefix_and_ignores_out_of_scope_objects() {
        let config = S3CompatibleConfig::new(
            "https://example.com",
            "bindhub-alpha",
            "auto",
            Some("accounts/account-alpha/projects/project-bindhub"),
            S3CredentialsSource::default(),
        )
        .expect("config parses");
        let parsed: S3ListBucketResult = quick_xml::de::from_str(
            r#"
            <ListBucketResult>
              <Contents>
                <Key>accounts/account-alpha/projects/project-bindhub/encrypted/blobs/a</Key>
                <Size>10</Size>
              </Contents>
              <Contents>
                <Key>accounts/account-alpha/projects/project-other/encrypted/blobs/b</Key>
                <Size>20</Size>
              </Contents>
            </ListBucketResult>
            "#,
        )
        .expect("list response parses");

        let objects = parsed
            .contents
            .into_iter()
            .filter_map(|object| {
                config
                    .strip_config_prefix(&object.key)
                    .map(|relative| ObjectMetadata {
                        key: ObjectKey::new(relative).expect("relative key parses"),
                        size_bytes: object.size,
                    })
            })
            .collect::<Vec<_>>();

        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].key.as_str(), "encrypted/blobs/a");
        assert_eq!(objects[0].size_bytes, 10);
    }
}

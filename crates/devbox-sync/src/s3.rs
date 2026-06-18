use crate::{ObjectKey, ObjectMetadata, PutOutcome, RemoteBlobProvider, SyncError, SyncResult};
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
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

    fn send_with_headers(
        &self,
        method: &str,
        key: &ObjectKey,
        body: Option<&[u8]>,
        extra_headers: &[(&str, &str)],
    ) -> SyncResult<S3Response> {
        let url = self.config.object_url(key);
        let body = body.unwrap_or_default();
        let payload_hash = sha256_hex(body);
        let signed = sign_request(
            method,
            &url,
            &payload_hash,
            &self.config.region,
            &self.credentials,
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
        if let Some(existing) = self.get(key)? {
            if existing == bytes {
                return Ok(PutOutcome {
                    uploaded: false,
                    size_bytes: existing.len() as u64,
                });
            }
            return Err(SyncError::RemoteObjectAlreadyExists { key: key.clone() });
        }

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
}

#[derive(Debug)]
struct S3Response {
    status: u16,
    bytes: Vec<u8>,
    content_length: Option<u64>,
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

    let mut canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{amz_date}\n");
    let mut signed_headers = "host;x-amz-content-sha256;x-amz-date".to_string();
    if let Some(token) = &credentials.session_token {
        canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
        signed_headers.push_str(";x-amz-security-token");
    }

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
            "devbox-alpha",
            "auto",
            Some("accounts/acct/projects"),
            S3CredentialsSource::env(
                "DEVBOX_R2_ACCESS_KEY_ID",
                "DEVBOX_R2_SECRET_ACCESS_KEY",
                Some("DEVBOX_R2_SESSION_TOKEN"),
            )
            .expect("credential source parses"),
        )
        .expect("config parses");

        let redacted = config.redacted().to_string();

        assert!(redacted.contains("endpoint_host=account.r2.cloudflarestorage.com"));
        assert!(redacted.contains("bucket=devbox-alpha"));
        assert!(redacted.contains("region=auto"));
        assert!(redacted.contains("prefix=accounts/acct/projects"));
        assert!(redacted.contains("access_key_env=DEVBOX_R2_ACCESS_KEY_ID"));
        assert!(redacted.contains("secret_key_env=DEVBOX_R2_SECRET_ACCESS_KEY"));
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
            "DEVBOX_TEST_MISSING_ACCESS_KEY",
            "DEVBOX_TEST_MISSING_SECRET_KEY",
            None::<String>,
        )
        .expect("source parses");

        let error = source.load().expect_err("missing env vars fail");
        let message = error.to_string();

        assert!(message.contains("DEVBOX_TEST_MISSING_ACCESS_KEY"));
        assert!(!message.contains("DEVBOX_TEST_MISSING_SECRET_KEY_VALUE"));
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
            OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("timestamp parses"),
        )
        .expect("request signs");

        assert!(signed.authorization.contains("Credential=AKIDEXAMPLE/"));
        assert!(!signed.authorization.contains("wJalrXUtnFEMI"));
    }
}

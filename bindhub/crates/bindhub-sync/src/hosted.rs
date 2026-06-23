use crate::{ObjectKey, ObjectMetadata, PutOutcome, RemoteBlobProvider, SyncError, SyncResult};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use serde::Deserialize;
use std::env;
use std::fmt;
use std::io::Read;
use ureq::{Agent, Error as UreqError};
use url::Url;

const API_ENCODE_SET: &AsciiSet = &CONTROLS
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

#[derive(Clone, PartialEq, Eq)]
pub struct HostedObjectTransferConfig {
    api: Url,
    project_id: String,
    lease_id: String,
    session_token_env: String,
}

impl HostedObjectTransferConfig {
    pub fn new(
        api: impl AsRef<str>,
        project_id: impl Into<String>,
        lease_id: impl Into<String>,
        session_token_env: impl Into<String>,
    ) -> SyncResult<Self> {
        Ok(Self {
            api: parse_api_endpoint(api.as_ref())?,
            project_id: validate_api_segment(project_id.into(), "project id")?,
            lease_id: validate_api_segment(lease_id.into(), "lease id")?,
            session_token_env: validate_env_name(session_token_env.into(), "session token env")?,
        })
    }

    pub fn api(&self) -> &Url {
        &self.api
    }

    pub fn project_id(&self) -> &str {
        &self.project_id
    }

    pub fn lease_id(&self) -> &str {
        &self.lease_id
    }

    pub fn session_token_env(&self) -> &str {
        &self.session_token_env
    }

    pub fn redacted(&self) -> HostedRedactedConfig {
        HostedRedactedConfig {
            api_host: self
                .api
                .host_str()
                .map(str::to_string)
                .unwrap_or_else(|| "-".to_string()),
            project_id: self.project_id.clone(),
            lease_id: self.lease_id.clone(),
            session_token_env: self.session_token_env.clone(),
        }
    }

    fn object_url(&self, key: &ObjectKey) -> String {
        let mut url = self.api.clone();
        url.set_path(&format!(
            "/v1/projects/{}/object-access/{}/object",
            encode_api_segment(&self.project_id),
            encode_api_segment(&self.lease_id)
        ));
        url.set_query(None);
        url.query_pairs_mut().append_pair("key", key.as_str());
        url.to_string()
    }

    fn list_url(&self, prefix: Option<&ObjectKey>) -> String {
        let mut url = self.api.clone();
        url.set_path(&format!(
            "/v1/projects/{}/object-access/{}/objects",
            encode_api_segment(&self.project_id),
            encode_api_segment(&self.lease_id)
        ));
        url.set_query(None);
        if let Some(prefix) = prefix {
            url.query_pairs_mut().append_pair("prefix", prefix.as_str());
        }
        url.to_string()
    }
}

impl fmt::Debug for HostedObjectTransferConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.redacted().fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostedRedactedConfig {
    pub api_host: String,
    pub project_id: String,
    pub lease_id: String,
    pub session_token_env: String,
}

impl fmt::Display for HostedRedactedConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "kind=hosted-object-transfer api_host={} project_id={} lease_id={} session_token_env={}",
            self.api_host, self.project_id, self.lease_id, self.session_token_env
        )
    }
}

#[derive(Clone)]
pub struct HostedObjectTransferProvider {
    config: HostedObjectTransferConfig,
    session_token: String,
    agent: Agent,
}

impl fmt::Debug for HostedObjectTransferProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostedObjectTransferProvider")
            .field("config", &self.config)
            .field("session_token", &"<redacted>")
            .field("agent", &"<ureq-agent>")
            .finish()
    }
}

impl HostedObjectTransferProvider {
    pub fn from_env(config: HostedObjectTransferConfig) -> SyncResult<Self> {
        let session_token = env::var(config.session_token_env()).map_err(|_| {
            SyncError::RemoteCredentials(format!(
                "session token env var {} is not set",
                config.session_token_env()
            ))
        })?;
        if session_token.trim().is_empty() {
            return Err(SyncError::RemoteCredentials(format!(
                "session token env var {} is empty",
                config.session_token_env()
            )));
        }
        Ok(Self::new(config, session_token))
    }

    pub fn new(config: HostedObjectTransferConfig, session_token: impl Into<String>) -> Self {
        Self {
            config,
            session_token: session_token.into(),
            agent: Agent::new(),
        }
    }

    pub fn config(&self) -> &HostedObjectTransferConfig {
        &self.config
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        self.agent
            .request(method, url)
            .set("authorization", &format!("Bearer {}", self.session_token))
    }

    fn call_bytes(
        &self,
        method: &str,
        url: &str,
        body: Option<&[u8]>,
    ) -> SyncResult<HostedResponse> {
        let response = match (method, body) {
            ("PUT", Some(bytes)) => self
                .request(method, url)
                .set("content-type", "application/octet-stream")
                .send_bytes(bytes),
            _ => self.request(method, url).call(),
        };

        match response {
            Ok(response) => hosted_response(method, response),
            Err(UreqError::Status(status, response)) => {
                let mut response = hosted_response(method, response)?;
                response.status = status;
                Ok(response)
            }
            Err(UreqError::Transport(_)) => Err(SyncError::RemoteTransport(
                "hosted object transfer transport failed".to_string(),
            )),
        }
    }
}

impl RemoteBlobProvider for HostedObjectTransferProvider {
    fn put(&self, key: &ObjectKey, bytes: &[u8]) -> SyncResult<PutOutcome> {
        let url = self.config.object_url(key);
        let response = self.call_bytes("PUT", &url, Some(bytes))?;
        match response.status {
            200 | 201 => {
                let outcome: HostedPutResponse = serde_json::from_slice(&response.bytes)
                    .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
                Ok(PutOutcome {
                    uploaded: outcome.uploaded,
                    size_bytes: outcome.size_bytes,
                })
            }
            409 | 412 => Err(SyncError::RemoteObjectAlreadyExists { key: key.clone() }),
            status => Err(SyncError::RemoteTransport(format!(
                "hosted object PUT failed with HTTP status {status}"
            ))),
        }
    }

    fn get(&self, key: &ObjectKey) -> SyncResult<Option<Vec<u8>>> {
        let url = self.config.object_url(key);
        let response = self.call_bytes("GET", &url, None)?;
        match response.status {
            200 => Ok(Some(response.bytes)),
            404 => Ok(None),
            status => Err(SyncError::RemoteTransport(format!(
                "hosted object GET failed with HTTP status {status}"
            ))),
        }
    }

    fn head(&self, key: &ObjectKey) -> SyncResult<Option<ObjectMetadata>> {
        let url = self.config.object_url(key);
        let response = self.call_bytes("HEAD", &url, None)?;
        match response.status {
            200 => {
                let size_bytes = response
                    .object_size
                    .or(response.content_length)
                    .unwrap_or_default();
                Ok(Some(ObjectMetadata {
                    key: key.clone(),
                    size_bytes,
                }))
            }
            404 => Ok(None),
            status => Err(SyncError::RemoteTransport(format!(
                "hosted object HEAD failed with HTTP status {status}"
            ))),
        }
    }

    fn list(&self, prefix: Option<&ObjectKey>) -> SyncResult<Vec<ObjectMetadata>> {
        let url = self.config.list_url(prefix);
        let response = self.call_bytes("GET", &url, None)?;
        match response.status {
            200 => {
                let response: HostedListResponse = serde_json::from_slice(&response.bytes)
                    .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
                response
                    .objects
                    .into_iter()
                    .map(|object| {
                        Ok(ObjectMetadata {
                            key: ObjectKey::new(object.key)?,
                            size_bytes: object.size_bytes,
                        })
                    })
                    .collect()
            }
            status => Err(SyncError::RemoteTransport(format!(
                "hosted object LIST failed with HTTP status {status}"
            ))),
        }
    }
}

#[derive(Debug)]
struct HostedResponse {
    status: u16,
    bytes: Vec<u8>,
    content_length: Option<u64>,
    object_size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct HostedPutResponse {
    uploaded: bool,
    size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct HostedListResponse {
    objects: Vec<HostedObjectMetadata>,
}

#[derive(Debug, Deserialize)]
struct HostedObjectMetadata {
    key: String,
    size_bytes: u64,
}

fn hosted_response(method: &str, response: ureq::Response) -> SyncResult<HostedResponse> {
    let status = response.status();
    let content_length = response
        .header("content-length")
        .and_then(|value| value.parse::<u64>().ok());
    let object_size = response
        .header("x-bindhub-object-size")
        .and_then(|value| value.parse::<u64>().ok());
    let bytes = if method == "HEAD" {
        Vec::new()
    } else {
        read_response_bytes(response)?
    };
    Ok(HostedResponse {
        status,
        bytes,
        content_length,
        object_size,
    })
}

fn read_response_bytes(response: ureq::Response) -> SyncResult<Vec<u8>> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| SyncError::RemoteTransport(error.to_string()))?;
    Ok(bytes)
}

fn parse_api_endpoint(value: &str) -> SyncResult<Url> {
    let url = Url::parse(value)
        .map_err(|_| SyncError::RemoteConfig("hosted object API must be a URL".to_string()))?;
    match url.scheme() {
        "http" | "https" => {}
        _ => {
            return Err(SyncError::RemoteConfig(
                "hosted object API must use http or https".to_string(),
            ))
        }
    }
    if url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() {
        return Err(SyncError::RemoteConfig(
            "hosted object API must include a host and no userinfo".to_string(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(SyncError::RemoteConfig(
            "hosted object API must not include query or fragment".to_string(),
        ));
    }
    Ok(url)
}

fn validate_api_segment(value: String, label: &'static str) -> SyncResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed
            .chars()
            .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
    {
        return Err(SyncError::RemoteConfig(format!(
            "{label} must be a safe hosted API path segment"
        )));
    }
    Ok(trimmed.to_string())
}

fn validate_env_name(value: String, label: &'static str) -> SyncResult<String> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(SyncError::RemoteConfig(format!(
            "{label} must be an environment variable name"
        )));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(SyncError::RemoteConfig(format!(
            "{label} must be an environment variable name"
        )));
    }
    Ok(value)
}

fn encode_api_segment(value: &str) -> String {
    utf8_percent_encode(value, API_ENCODE_SET).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_provider_debug_redacts_bearer_session_token() {
        let provider = HostedObjectTransferProvider::new(
            HostedObjectTransferConfig::new(
                "https://metadata.example",
                "project-bindhub",
                "lease-alpha",
                "BINDHUB_SESSION_TOKEN",
            )
            .expect("hosted config validates"),
            "raw-hosted-session-token-should-not-appear",
        );

        let debug = format!("{provider:?}");

        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("HostedObjectTransferProvider"));
        assert!(!debug.contains("raw-hosted-session-token-should-not-appear"));
        assert!(!debug.contains("Bearer"));
    }
}

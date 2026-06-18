//! Encrypted immutable blob transport for Devbox sync foundations.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use devbox_core::BlobId;
use devbox_store::{BlobCache, BlobCacheError};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

const OBJECTS_DIR: &str = "objects";
const TEMP_DIR: &str = "tmp";
const ENVELOPE_MAGIC: &[u8] = b"devbox-sync-v1\n";
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

#[derive(Debug)]
pub enum SyncError {
    Io(io::Error),
    BlobCache(BlobCacheError),
    InvalidObjectKey(String),
    InvalidKey(String),
    Encryption,
    Decryption,
    MissingRemoteObject(ObjectKey),
    RemoteObjectAlreadyExists { key: ObjectKey },
    BlobIdMismatch { expected: BlobId, actual: BlobId },
}

impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::BlobCache(error) => write!(f, "{error}"),
            Self::InvalidObjectKey(message) => write!(f, "invalid remote object key: {message}"),
            Self::InvalidKey(message) => write!(f, "invalid sync key: {message}"),
            Self::Encryption => f.write_str("remote object encryption failed"),
            Self::Decryption => f.write_str("remote object decryption failed"),
            Self::MissingRemoteObject(key) => write!(f, "remote object not found: {key}"),
            Self::RemoteObjectAlreadyExists { key } => {
                write!(
                    f,
                    "remote object already exists with different bytes: {key}"
                )
            }
            Self::BlobIdMismatch { expected, actual } => {
                write!(
                    f,
                    "downloaded blob hash mismatch: expected {expected}, got {actual}"
                )
            }
        }
    }
}

impl std::error::Error for SyncError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::BlobCache(error) => Some(error),
            Self::InvalidObjectKey(_)
            | Self::InvalidKey(_)
            | Self::Encryption
            | Self::Decryption
            | Self::MissingRemoteObject(_)
            | Self::RemoteObjectAlreadyExists { .. }
            | Self::BlobIdMismatch { .. } => None,
        }
    }
}

impl From<io::Error> for SyncError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<BlobCacheError> for SyncError {
    fn from(error: BlobCacheError) -> Self {
        Self::BlobCache(error)
    }
}

pub type SyncResult<T> = Result<T, SyncError>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    pub fn new(value: impl Into<String>) -> SyncResult<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SyncError::InvalidObjectKey(
                "object key cannot be empty".to_string(),
            ));
        }
        if value.contains('\\') {
            return Err(SyncError::InvalidObjectKey(
                "object key must use '/' separators".to_string(),
            ));
        }
        if value.starts_with('/') {
            return Err(SyncError::InvalidObjectKey(
                "object key must be relative".to_string(),
            ));
        }

        let path = Path::new(&value);
        for component in path.components() {
            match component {
                Component::Normal(_) => {}
                Component::CurDir
                | Component::ParentDir
                | Component::RootDir
                | Component::Prefix(_) => {
                    return Err(SyncError::InvalidObjectKey(
                        "object key must contain only normal relative path components".to_string(),
                    ));
                }
            }
        }
        if value.split('/').any(str::is_empty) {
            return Err(SyncError::InvalidObjectKey(
                "object key cannot contain empty path components".to_string(),
            ));
        }

        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub key: ObjectKey,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutOutcome {
    pub uploaded: bool,
    pub size_bytes: u64,
}

pub trait RemoteBlobProvider {
    fn put(&self, key: &ObjectKey, bytes: &[u8]) -> SyncResult<PutOutcome>;
    fn get(&self, key: &ObjectKey) -> SyncResult<Option<Vec<u8>>>;
    fn head(&self, key: &ObjectKey) -> SyncResult<Option<ObjectMetadata>>;
}

#[derive(Debug, Clone)]
pub struct LocalFilesystemBlobProvider {
    root: PathBuf,
}

impl LocalFilesystemBlobProvider {
    pub fn open(root: impl AsRef<Path>) -> SyncResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(OBJECTS_DIR))?;
        fs::create_dir_all(root.join(TEMP_DIR))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_for(&self, key: &ObjectKey) -> PathBuf {
        key.as_str()
            .split('/')
            .fold(self.root.join(OBJECTS_DIR), |path, segment| {
                path.join(segment)
            })
    }

    fn create_temp_file(&self) -> SyncResult<(File, PathBuf)> {
        let temp_dir = self.root.join(TEMP_DIR);
        fs::create_dir_all(&temp_dir)?;
        for _ in 0..100 {
            let path = temp_dir.join(temp_file_name());
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((file, path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique remote provider temp file",
        )
        .into())
    }
}

impl RemoteBlobProvider for LocalFilesystemBlobProvider {
    fn put(&self, key: &ObjectKey, bytes: &[u8]) -> SyncResult<PutOutcome> {
        let final_path = self.path_for(key);
        if let Some(outcome) = existing_put_outcome(&final_path, key, bytes)? {
            return Ok(outcome);
        }

        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let (mut temp_file, temp_path) = self.create_temp_file()?;
        if let Err(error) = temp_file
            .write_all(bytes)
            .and_then(|_| temp_file.sync_all())
        {
            cleanup_temp_file(&temp_path);
            return Err(error.into());
        }
        drop(temp_file);

        let result = commit_new_file_no_overwrite(&temp_path, &final_path);
        cleanup_temp_file(&temp_path);
        match result {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if let Some(outcome) = existing_put_outcome(&final_path, key, bytes)? {
                    return Ok(outcome);
                }

                return Err(error.into());
            }
            Err(error) => return Err(error.into()),
        }

        Ok(PutOutcome {
            uploaded: true,
            size_bytes: bytes.len() as u64,
        })
    }

    fn get(&self, key: &ObjectKey) -> SyncResult<Option<Vec<u8>>> {
        let path = self.path_for(key);
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    fn head(&self, key: &ObjectKey) -> SyncResult<Option<ObjectMetadata>> {
        let path = self.path_for(key);
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => Ok(Some(ObjectMetadata {
                key: key.clone(),
                size_bytes: metadata.len(),
            })),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncKey([u8; KEY_LEN]);

impl SyncKey {
    pub fn from_hex(value: &str) -> SyncResult<Self> {
        if value.len() != KEY_LEN * 2 {
            return Err(SyncError::InvalidKey(format!(
                "expected {} hex characters",
                KEY_LEN * 2
            )));
        }

        let mut bytes = [0_u8; KEY_LEN];
        for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_value(chunk[0]).ok_or_else(|| {
                SyncError::InvalidKey("sync key contains a non-hex character".to_string())
            })?;
            let low = hex_value(chunk[1]).ok_or_else(|| {
                SyncError::InvalidKey("sync key contains a non-hex character".to_string())
            })?;
            bytes[index] = (high << 4) | low;
        }

        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedBlob {
    pub object_key: ObjectKey,
    pub plaintext_bytes: u64,
    pub remote_bytes: u64,
    pub uploaded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadedBlob {
    pub object_key: ObjectKey,
    pub plaintext_bytes: u64,
    pub remote_bytes: u64,
}

pub fn encrypted_blob_object_key(blob_id: &BlobId) -> ObjectKey {
    ObjectKey::new(format!(
        "encrypted/blobs/b3/{}/{}/{}",
        &blob_id.as_str()[0..2],
        &blob_id.as_str()[2..4],
        blob_id
    ))
    .expect("blob ids produce valid object keys")
}

pub fn upload_blob_from_cache(
    cache: &BlobCache,
    provider: &impl RemoteBlobProvider,
    key: &SyncKey,
    blob_id: &BlobId,
    object_key: &ObjectKey,
) -> SyncResult<UploadedBlob> {
    let plaintext = cache.read(blob_id)?;
    if let Some(existing) = provider.get(object_key)? {
        let existing_plaintext = decrypt_payload(key, object_key, &existing)?;
        if existing_plaintext == plaintext {
            return Ok(UploadedBlob {
                object_key: object_key.clone(),
                plaintext_bytes: plaintext.len() as u64,
                remote_bytes: existing.len() as u64,
                uploaded: false,
            });
        }

        return Err(SyncError::RemoteObjectAlreadyExists {
            key: object_key.clone(),
        });
    }

    let encrypted = encrypt_payload(key, object_key, &plaintext)?;
    let outcome = provider.put(object_key, &encrypted)?;

    Ok(UploadedBlob {
        object_key: object_key.clone(),
        plaintext_bytes: plaintext.len() as u64,
        remote_bytes: outcome.size_bytes,
        uploaded: outcome.uploaded,
    })
}

pub fn download_blob_to_cache(
    cache: &BlobCache,
    provider: &impl RemoteBlobProvider,
    key: &SyncKey,
    expected_blob_id: &BlobId,
    object_key: &ObjectKey,
) -> SyncResult<DownloadedBlob> {
    let encrypted = provider
        .get(object_key)?
        .ok_or_else(|| SyncError::MissingRemoteObject(object_key.clone()))?;
    let plaintext = decrypt_payload(key, object_key, &encrypted)?;
    let actual_blob_id = BlobId::from_blake3_hex(blake3::hash(&plaintext).to_hex().to_string())
        .expect("BLAKE3 returns a 64-character hex digest");
    if &actual_blob_id != expected_blob_id {
        return Err(SyncError::BlobIdMismatch {
            expected: expected_blob_id.clone(),
            actual: actual_blob_id,
        });
    }
    cache.write_bytes(&plaintext)?;

    Ok(DownloadedBlob {
        object_key: object_key.clone(),
        plaintext_bytes: plaintext.len() as u64,
        remote_bytes: encrypted.len() as u64,
    })
}

pub fn encrypt_payload(
    sync_key: &SyncKey,
    object_key: &ObjectKey,
    plaintext: &[u8],
) -> SyncResult<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&sync_key.0));
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| SyncError::Encryption)?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: object_key.as_str().as_bytes(),
            },
        )
        .map_err(|_| SyncError::Encryption)?;

    let mut envelope = Vec::with_capacity(ENVELOPE_MAGIC.len() + NONCE_LEN + ciphertext.len());
    envelope.extend_from_slice(ENVELOPE_MAGIC);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

pub fn decrypt_payload(
    sync_key: &SyncKey,
    object_key: &ObjectKey,
    envelope: &[u8],
) -> SyncResult<Vec<u8>> {
    if envelope.len() < ENVELOPE_MAGIC.len() + NONCE_LEN
        || &envelope[..ENVELOPE_MAGIC.len()] != ENVELOPE_MAGIC
    {
        return Err(SyncError::Decryption);
    }

    let nonce_start = ENVELOPE_MAGIC.len();
    let ciphertext_start = nonce_start + NONCE_LEN;
    let nonce = &envelope[nonce_start..ciphertext_start];
    let ciphertext = &envelope[ciphertext_start..];
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&sync_key.0));

    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: object_key.as_str().as_bytes(),
            },
        )
        .map_err(|_| SyncError::Decryption)
}

fn temp_file_name() -> String {
    let mut random = [0_u8; 8];
    let _ = getrandom::getrandom(&mut random);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!(
        "remote-{}-{nanos}-{}.tmp",
        process::id(),
        hex_encode(&random)
    )
}

fn cleanup_temp_file(path: &Path) {
    let _ = fs::remove_file(path);
}

fn existing_put_outcome(
    final_path: &Path,
    key: &ObjectKey,
    bytes: &[u8],
) -> SyncResult<Option<PutOutcome>> {
    match fs::read(final_path) {
        Ok(existing) if existing == bytes => Ok(Some(PutOutcome {
            uploaded: false,
            size_bytes: existing.len() as u64,
        })),
        Ok(_) => Err(SyncError::RemoteObjectAlreadyExists { key: key.clone() }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn commit_new_file_no_overwrite(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::hard_link(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists || destination.exists() => {
            Err(io::Error::new(io::ErrorKind::AlreadyExists, error))
        }
        Err(error) => Err(error),
    }
}

fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn local_provider_put_is_immutable_and_idempotent() {
        let dir = tempfile::tempdir().expect("temp dir");
        let provider = LocalFilesystemBlobProvider::open(dir.path()).expect("provider opens");
        let key = ObjectKey::new("account/blob").expect("key parses");

        let first = provider.put(&key, b"encrypted").expect("first put works");
        let second = provider
            .put(&key, b"encrypted")
            .expect("same put is idempotent");
        let changed = provider.put(&key, b"different");

        assert!(first.uploaded);
        assert!(!second.uploaded);
        assert!(matches!(
            changed,
            Err(SyncError::RemoteObjectAlreadyExists { key: existing }) if existing == key
        ));
        assert_eq!(
            provider.get(&key).expect("get works"),
            Some(b"encrypted".to_vec())
        );
    }

    #[test]
    fn no_overwrite_commit_refuses_to_replace_existing_object_bytes() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source = dir.path().join("source.tmp");
        let destination = dir.path().join("object");
        fs::write(&source, b"new encrypted bytes").expect("source writes");
        fs::write(&destination, b"existing encrypted bytes").expect("destination writes");

        let error = commit_new_file_no_overwrite(&source, &destination)
            .expect_err("existing destination is not replaced");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read(&destination).expect("destination reads"),
            b"existing encrypted bytes"
        );
        assert_eq!(
            fs::read(&source).expect("source remains"),
            b"new encrypted bytes"
        );
    }

    #[test]
    fn local_provider_rejects_unsafe_object_keys() {
        assert!(ObjectKey::new("../escape").is_err());
        assert!(ObjectKey::new("nested/../escape").is_err());
        assert!(ObjectKey::new("/absolute").is_err());
        assert!(ObjectKey::new("double//slash").is_err());
        assert!(ObjectKey::new("windows\\path").is_err());
    }

    #[test]
    fn encrypted_round_trip_keeps_plaintext_out_of_remote_storage() {
        let dir = tempfile::tempdir().expect("temp dir");
        let remote = dir.path().join("remote");
        let provider = LocalFilesystemBlobProvider::open(&remote).expect("provider opens");
        let cache = BlobCache::open(dir.path().join("cache")).expect("cache opens");
        let restored_cache = BlobCache::open(dir.path().join("restored")).expect("cache opens");
        let plaintext = b"plain source bytes should not appear remotely";
        let blob = cache.write_bytes(plaintext).expect("blob writes");
        let key = SyncKey::from_bytes([7; 32]);
        let object_key = encrypted_blob_object_key(blob.id());

        let upload = upload_blob_from_cache(&cache, &provider, &key, blob.id(), &object_key)
            .expect("upload works");
        let second_upload = upload_blob_from_cache(&cache, &provider, &key, blob.id(), &object_key)
            .expect("same blob upload is idempotent");
        let remote_bytes = fs::read(provider.path_for(&object_key)).expect("remote object reads");
        let download =
            download_blob_to_cache(&restored_cache, &provider, &key, blob.id(), &object_key)
                .expect("download works");

        assert!(upload.uploaded);
        assert!(!second_upload.uploaded);
        assert_eq!(upload.plaintext_bytes, plaintext.len() as u64);
        assert!(!remote_bytes
            .windows(plaintext.len())
            .any(|window| window == plaintext));
        assert_eq!(download.plaintext_bytes, plaintext.len() as u64);
        assert_eq!(
            restored_cache.read(blob.id()).expect("blob reads"),
            plaintext
        );
    }

    #[test]
    fn mismatched_download_does_not_commit_unexpected_blob_to_cache() {
        let dir = tempfile::tempdir().expect("temp dir");
        let provider =
            LocalFilesystemBlobProvider::open(dir.path().join("remote")).expect("provider opens");
        let target_cache = BlobCache::open(dir.path().join("target-cache")).expect("cache opens");
        let key = SyncKey::from_bytes([11; 32]);
        let actual_plaintext = b"actual unexpected plaintext";
        let actual_blob_id =
            BlobId::from_blake3_hex(blake3::hash(actual_plaintext).to_hex().to_string())
                .expect("valid actual blob id");
        let expected_blob_id = BlobId::from_blake3_hex(
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        )
        .expect("valid expected blob id");
        let object_key = encrypted_blob_object_key(&expected_blob_id);
        let encrypted =
            encrypt_payload(&key, &object_key, actual_plaintext).expect("encryption works");
        provider
            .put(&object_key, &encrypted)
            .expect("remote object writes");

        let error = download_blob_to_cache(
            &target_cache,
            &provider,
            &key,
            &expected_blob_id,
            &object_key,
        )
        .expect_err("mismatched blob id fails");

        assert!(matches!(
            error,
            SyncError::BlobIdMismatch { expected, actual }
                if expected == expected_blob_id && actual == actual_blob_id
        ));
        assert!(!target_cache.exists(&expected_blob_id));
        assert!(!target_cache.exists(&actual_blob_id));
        assert_eq!(cache_file_count(target_cache.root()), 0);
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let object_key = ObjectKey::new("encrypted/blob").expect("key parses");
        let first_key = SyncKey::from_bytes([1; 32]);
        let second_key = SyncKey::from_bytes([2; 32]);
        let encrypted =
            encrypt_payload(&first_key, &object_key, b"secret").expect("encryption works");

        let error =
            decrypt_payload(&second_key, &object_key, &encrypted).expect_err("wrong key fails");

        assert!(matches!(error, SyncError::Decryption));
    }

    #[test]
    fn missing_remote_object_is_reported_without_creating_cache_blob() {
        let dir = tempfile::tempdir().expect("temp dir");
        let provider =
            LocalFilesystemBlobProvider::open(dir.path().join("remote")).expect("provider opens");
        let cache = BlobCache::open(dir.path().join("cache")).expect("cache opens");
        let blob_id = BlobId::from_blake3_hex(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .expect("valid blob id");
        let object_key = encrypted_blob_object_key(&blob_id);
        let key = SyncKey::from_bytes([9; 32]);

        let error = download_blob_to_cache(&cache, &provider, &key, &blob_id, &object_key)
            .expect_err("missing object fails");

        assert!(matches!(
            error,
            SyncError::MissingRemoteObject(missing) if missing == object_key
        ));
        assert!(!cache.exists(&blob_id));
    }

    fn cache_file_count(root: &Path) -> usize {
        count_files(&root.join("blobs"))
    }

    fn count_files(path: &Path) -> usize {
        let mut count = 0;
        let mut stack = vec![path.to_path_buf()];

        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(path).expect("directory reads") {
                let entry = entry.expect("directory entry reads");
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    count += 1;
                }
            }
        }

        count
    }
}

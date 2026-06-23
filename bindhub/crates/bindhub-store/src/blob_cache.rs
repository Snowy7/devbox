use bindhub_core::BlobId;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const HASH_ALGORITHM_DIR: &str = "b3";
const OBJECTS_DIR: &str = "blobs";
const TEMP_DIR: &str = "tmp";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum BlobCacheError {
    Io(io::Error),
    MissingBlob { id: BlobId, path: PathBuf },
}

impl fmt::Display for BlobCacheError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::MissingBlob { id, path } => {
                write!(f, "blob {id} is missing at {}", path.display())
            }
        }
    }
}

impl std::error::Error for BlobCacheError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::MissingBlob { .. } => None,
        }
    }
}

impl From<io::Error> for BlobCacheError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type BlobCacheResult<T> = Result<T, BlobCacheError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    id: BlobId,
    path: PathBuf,
    size_bytes: u64,
}

impl BlobRef {
    pub fn id(&self) -> &BlobId {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn object_ref(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            OBJECTS_DIR,
            HASH_ALGORITHM_DIR,
            &self.id.as_str()[0..2],
            &self.id.as_str()[2..4],
            self.id
        )
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

#[derive(Debug, Clone)]
pub struct BlobCache {
    root: PathBuf,
}

impl BlobCache {
    pub fn open(root: impl AsRef<Path>) -> BlobCacheResult<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(OBJECTS_DIR).join(HASH_ALGORITHM_DIR))?;
        fs::create_dir_all(root.join(TEMP_DIR))?;

        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_bytes(&self, bytes: impl AsRef<[u8]>) -> BlobCacheResult<BlobRef> {
        self.write_reader(bytes.as_ref())
    }

    pub fn write_file(&self, path: impl AsRef<Path>) -> BlobCacheResult<BlobRef> {
        let file = File::open(path)?;
        self.write_reader(BufReader::new(file))
    }

    pub fn read(&self, id: &BlobId) -> BlobCacheResult<Vec<u8>> {
        let path = self.path_for(id);
        match fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(BlobCacheError::MissingBlob {
                    id: id.clone(),
                    path,
                })
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn exists(&self, id: &BlobId) -> bool {
        self.path_for(id).is_file()
    }

    pub fn path_for(&self, id: &BlobId) -> PathBuf {
        self.root
            .join(OBJECTS_DIR)
            .join(HASH_ALGORITHM_DIR)
            .join(&id.as_str()[0..2])
            .join(&id.as_str()[2..4])
            .join(id.as_str())
    }

    fn write_reader(&self, mut reader: impl Read) -> BlobCacheResult<BlobRef> {
        let (mut temp_file, temp_path) = self.create_temp_file()?;
        let mut hasher = blake3::Hasher::new();
        let mut size_bytes = 0;
        let mut buffer = [0; 64 * 1024];

        loop {
            let bytes_read = match reader.read(&mut buffer) {
                Ok(bytes_read) => bytes_read,
                Err(error) => {
                    cleanup_temp_file(&temp_path);
                    return Err(error.into());
                }
            };

            if bytes_read == 0 {
                break;
            }

            hasher.update(&buffer[..bytes_read]);
            size_bytes += bytes_read as u64;

            if let Err(error) = temp_file.write_all(&buffer[..bytes_read]) {
                cleanup_temp_file(&temp_path);
                return Err(error.into());
            }
        }

        if let Err(error) = temp_file.flush().and_then(|_| temp_file.sync_all()) {
            cleanup_temp_file(&temp_path);
            return Err(error.into());
        }

        drop(temp_file);

        let id = BlobId::from_blake3_hex(hasher.finalize().to_hex().to_string())
            .expect("BLAKE3 returns a 64-character hex digest");
        let final_path = self.path_for(&id);
        fs::create_dir_all(
            final_path
                .parent()
                .expect("blob paths are always nested below cache root"),
        )?;

        if final_path.exists() {
            cleanup_temp_file(&temp_path);
        } else if let Err(error) = fs::rename(&temp_path, &final_path) {
            if error.kind() == io::ErrorKind::AlreadyExists && final_path.exists() {
                cleanup_temp_file(&temp_path);
            } else {
                cleanup_temp_file(&temp_path);
                return Err(error.into());
            }
        }

        Ok(BlobRef {
            id,
            path: final_path,
            size_bytes,
        })
    }

    fn create_temp_file(&self) -> BlobCacheResult<(File, PathBuf)> {
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
            "could not create a unique blob cache temp file",
        )
        .into())
    }
}

fn temp_file_name() -> String {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();

    format!("blob-{}-{nanos}-{counter}.tmp", process::id())
}

fn cleanup_temp_file(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Deref;

    #[test]
    fn writes_and_reads_bytes_by_blake3_identity() {
        let cache = temp_cache();
        let content = b"hello from the local blob cache";

        let blob = cache.write_bytes(content).expect("blob writes");

        assert_eq!(blob.id().as_str(), expected_id(content).as_str());
        assert_eq!(blob.size_bytes(), content.len() as u64);
        assert!(cache.exists(blob.id()));
        assert_eq!(cache.read(blob.id()).expect("blob reads"), content);
    }

    #[test]
    fn duplicate_byte_writes_are_idempotent() {
        let cache = temp_cache();
        let content = b"same bytes, same object";

        let first = cache.write_bytes(content).expect("first write succeeds");
        let second = cache.write_bytes(content).expect("second write succeeds");

        assert_eq!(first.id(), second.id());
        assert_eq!(first.path(), second.path());
        assert_eq!(object_file_count(cache.root()), 1);
        assert_eq!(temp_file_count(cache.root()), 0);
    }

    #[test]
    fn writes_from_file_path() {
        let dir = tempfile::tempdir().expect("temp dir");
        let source_path = dir.path().join("source.txt");
        let content = b"file content can enter the cache";
        fs::write(&source_path, content).expect("source file writes");
        let cache = BlobCache::open(dir.path().join("cache")).expect("cache opens");

        let blob = cache.write_file(&source_path).expect("file writes");

        assert_eq!(blob.id().as_str(), expected_id(content).as_str());
        assert_eq!(cache.read(blob.id()).expect("blob reads"), content);
    }

    #[test]
    fn missing_blob_reports_id_and_path() {
        let cache = temp_cache();
        let id = expected_id(b"not written");

        let error = cache.read(&id).expect_err("blob is missing");

        assert!(matches!(
            error,
            BlobCacheError::MissingBlob {
                id: missing_id,
                path
            } if missing_id == id && path == cache.path_for(&id)
        ));
        assert!(!cache.exists(&id));
    }

    #[test]
    fn path_layout_is_sharded_by_hash_prefix() {
        let cache = temp_cache();
        let id = expected_id(b"layout");

        let path = cache.path_for(&id);
        let components = path_components_after_root(cache.root(), &path);

        assert_eq!(components[0], OBJECTS_DIR);
        assert_eq!(components[1], HASH_ALGORITHM_DIR);
        assert_eq!(components[2], &id.as_str()[0..2]);
        assert_eq!(components[3], &id.as_str()[2..4]);
        assert_eq!(components[4], id.as_str());

        let blob = cache.write_bytes(b"layout").expect("blob writes");
        assert_eq!(blob.path(), path);
        assert_eq!(
            blob.object_ref(),
            format!(
                "{}/{}/{}/{}/{}",
                OBJECTS_DIR,
                HASH_ALGORITHM_DIR,
                &id.as_str()[0..2],
                &id.as_str()[2..4],
                id
            )
        );
    }

    fn temp_cache() -> TestCache {
        TestCache::new()
    }

    struct TestCache {
        _dir: tempfile::TempDir,
        cache: BlobCache,
    }

    impl TestCache {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let cache = BlobCache::open(dir.path()).expect("cache opens");

            Self { _dir: dir, cache }
        }
    }

    impl Deref for TestCache {
        type Target = BlobCache;

        fn deref(&self) -> &Self::Target {
            &self.cache
        }
    }

    fn expected_id(content: &[u8]) -> BlobId {
        BlobId::from_blake3_hex(blake3::hash(content).to_hex().to_string())
            .expect("BLAKE3 returns valid blob ids")
    }

    fn object_file_count(root: &Path) -> usize {
        count_files(&root.join(OBJECTS_DIR))
    }

    fn temp_file_count(root: &Path) -> usize {
        count_files(&root.join(TEMP_DIR))
    }

    fn count_files(path: &Path) -> usize {
        let mut count = 0;
        let mut stack = vec![path.to_path_buf()];

        while let Some(path) = stack.pop() {
            for entry in fs::read_dir(path).expect("directory reads") {
                let entry = entry.expect("directory entry reads");
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    stack.push(entry_path);
                } else {
                    count += 1;
                }
            }
        }

        count
    }

    fn path_components_after_root<'a>(root: &Path, path: &'a Path) -> Vec<&'a str> {
        path.strip_prefix(root)
            .expect("path is inside root")
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .expect("test paths are UTF-8")
            })
            .collect()
    }
}

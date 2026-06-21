//! Loom pack-format boundary.
//!
//! The format uses a deterministic envelope: metadata rows, object
//! availability rows, and optional content-addressed object payload rows.
//! Compression is still `None`; the manifest keeps that field explicit so later
//! compression can fit without changing folder-state semantics.

use loom_core::{
    Checkpoint, CheckpointId, FileKind, FileVersion, FileVersionId, FolderEntry, FolderRevision,
    FolderRevisionId, LoomError, ObjectId, Pin, PinId, RevisionBoundary, SharedFolderId,
};
use std::fmt;
use std::path::{Component, Path, PathBuf};

const PACK_MAGIC: &str = "loom-pack-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackCompression {
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackPayloadAvailability {
    Inline,
    Remote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackObject {
    pub object_id: ObjectId,
    pub size_bytes: u64,
    pub compression: PackCompression,
    pub availability: PackPayloadAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackObjectPayload {
    pub object_id: ObjectId,
    pub compression: PackCompression,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    pub shared_folder_id: SharedFolderId,
    pub display_name: String,
    pub latest_revision_id: FolderRevisionId,
    pub objects: Vec<PackObject>,
}

impl PackManifest {
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoomPack {
    pub manifest: PackManifest,
    pub file_versions: Vec<FileVersion>,
    pub revisions: Vec<FolderRevision>,
    pub checkpoints: Vec<Checkpoint>,
    pub pins: Vec<Pin>,
    pub object_payloads: Vec<PackObjectPayload>,
}

impl LoomPack {
    pub fn new(
        shared_folder_id: SharedFolderId,
        display_name: impl Into<String>,
        latest_revision_id: FolderRevisionId,
        file_versions: Vec<FileVersion>,
        revisions: Vec<FolderRevision>,
        checkpoints: Vec<Checkpoint>,
        pins: Vec<Pin>,
        objects: Vec<PackObject>,
        object_payloads: Vec<PackObjectPayload>,
    ) -> Result<Self, PackError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() {
            return Err(PackError::InvalidFormat(
                "shared folder display name cannot be empty".to_string(),
            ));
        }
        validate_object_payloads(&objects, &object_payloads)?;

        Ok(Self {
            manifest: PackManifest {
                shared_folder_id,
                display_name,
                latest_revision_id,
                objects,
            },
            file_versions,
            revisions,
            checkpoints,
            pins,
            object_payloads,
        })
    }

    pub fn object_payload(&self, object_id: &ObjectId) -> Option<&PackObjectPayload> {
        self.object_payloads
            .iter()
            .find(|payload| &payload.object_id == object_id)
    }

    pub fn inline_object_count(&self) -> usize {
        self.object_payloads.len()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut rows = Vec::new();
        rows.push(PACK_MAGIC.to_string());
        rows.push(format!(
            "shared\t{}\t{}",
            encode_field(self.manifest.shared_folder_id.as_str()),
            encode_field(&self.manifest.display_name)
        ));
        rows.push(format!(
            "latest\t{}",
            encode_field(self.manifest.latest_revision_id.as_str())
        ));

        for version in &self.file_versions {
            rows.push(format!(
                "file\t{}\t{}\t{}\t{}\t{}\t{}",
                encode_field(version.id().as_str()),
                encode_field(&path_to_pack_string(version.path())),
                encode_field(file_kind_to_pack(version.kind())),
                encode_field(version.object_id().map(ObjectId::as_str).unwrap_or("-")),
                encode_field(
                    &version
                        .size_bytes()
                        .map(|size| size.to_string())
                        .unwrap_or_else(|| "-".to_string())
                ),
                encode_field(version.captured_at())
            ));
        }

        for revision in &self.revisions {
            rows.push(format!(
                "revision\t{}\t{}\t{}\t{}",
                encode_field(revision.id().as_str()),
                encode_field(
                    revision
                        .parent_id()
                        .map(FolderRevisionId::as_str)
                        .unwrap_or("-")
                ),
                encode_field(revision_boundary_to_pack(revision.boundary())),
                encode_field(revision.created_at())
            ));
            for entry in revision.entries() {
                rows.push(format!(
                    "entry\t{}\t{}\t{}",
                    encode_field(revision.id().as_str()),
                    encode_field(&path_to_pack_string(entry.path())),
                    encode_field(entry.file_version_id().as_str())
                ));
            }
        }

        for checkpoint in &self.checkpoints {
            rows.push(format!(
                "checkpoint\t{}\t{}\t{}\t{}",
                encode_field(checkpoint.id().as_str()),
                encode_field(checkpoint.revision_id().as_str()),
                encode_field(checkpoint.message()),
                encode_field(checkpoint.created_at())
            ));
        }

        for pin in &self.pins {
            rows.push(format!(
                "pin\t{}\t{}\t{}\t{}",
                encode_field(pin.id().as_str()),
                encode_field(pin.revision_id().as_str()),
                encode_field(pin.reason()),
                encode_field(pin.created_at())
            ));
        }

        for object in &self.manifest.objects {
            rows.push(format!(
                "object\t{}\t{}\t{}\t{}",
                encode_field(object.object_id.as_str()),
                object.size_bytes,
                compression_to_pack(object.compression.clone()),
                availability_to_pack(object.availability)
            ));
        }

        for payload in &self.object_payloads {
            rows.push(format!(
                "object-payload\t{}\t{}\t{}",
                encode_field(payload.object_id.as_str()),
                compression_to_pack(payload.compression.clone()),
                hex_encode(&payload.payload)
            ));
        }

        rows.push(String::new());
        rows.join("\n").into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PackError> {
        let contents = std::str::from_utf8(bytes)
            .map_err(|_| PackError::InvalidFormat("pack is not UTF-8".to_string()))?;
        let mut lines = contents.lines();
        let magic = lines
            .next()
            .ok_or_else(|| PackError::InvalidFormat("empty pack".to_string()))?;
        if magic != PACK_MAGIC {
            return Err(PackError::InvalidFormat(format!(
                "unknown pack magic {magic}"
            )));
        }

        let mut shared_folder_id = None;
        let mut display_name = None;
        let mut latest_revision_id = None;
        let mut file_versions = Vec::new();
        let mut revision_headers = Vec::new();
        let mut revision_entries = Vec::<(FolderRevisionId, FolderEntry)>::new();
        let mut checkpoints = Vec::new();
        let mut pins = Vec::new();
        let mut objects = Vec::new();
        let mut object_payloads = Vec::new();

        for (line_index, line) in lines.enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let fields = line.split('\t').collect::<Vec<_>>();
            match fields.first().copied() {
                Some("shared") => {
                    expect_fields(&fields, 3, line_index + 2)?;
                    shared_folder_id = Some(SharedFolderId::new(decode_field(fields[1])?)?);
                    display_name = Some(decode_field(fields[2])?);
                }
                Some("latest") => {
                    expect_fields(&fields, 2, line_index + 2)?;
                    latest_revision_id = Some(FolderRevisionId::new(decode_field(fields[1])?)?);
                }
                Some("file") => {
                    expect_fields(&fields, 7, line_index + 2)?;
                    let object_id = match decode_field(fields[4])?.as_str() {
                        "-" => None,
                        value => Some(ObjectId::from_blake3_hex(value.to_string())?),
                    };
                    let size_bytes = match decode_field(fields[5])?.as_str() {
                        "-" => None,
                        value => Some(value.parse::<u64>().map_err(|_| {
                            PackError::InvalidFormat(format!(
                                "line {} has invalid file size",
                                line_index + 2
                            ))
                        })?),
                    };
                    let kind = file_kind_from_pack(&decode_field(fields[3])?).ok_or_else(|| {
                        PackError::InvalidFormat(format!(
                            "line {} has unknown file kind",
                            line_index + 2
                        ))
                    })?;
                    file_versions.push(FileVersion::new(
                        FileVersionId::new(decode_field(fields[1])?)?,
                        pack_string_to_path(&decode_field(fields[2])?),
                        kind,
                        object_id,
                        size_bytes,
                        decode_field(fields[6])?,
                    )?);
                }
                Some("revision") => {
                    expect_fields(&fields, 5, line_index + 2)?;
                    let parent_id = match decode_field(fields[2])?.as_str() {
                        "-" => None,
                        value => Some(FolderRevisionId::new(value.to_string())?),
                    };
                    let boundary = revision_boundary_from_pack(&decode_field(fields[3])?)
                        .ok_or_else(|| {
                            PackError::InvalidFormat(format!(
                                "line {} has unknown revision boundary",
                                line_index + 2
                            ))
                        })?;
                    revision_headers.push(RevisionHeader {
                        id: FolderRevisionId::new(decode_field(fields[1])?)?,
                        parent_id,
                        boundary,
                        created_at: decode_field(fields[4])?,
                    });
                }
                Some("entry") => {
                    expect_fields(&fields, 4, line_index + 2)?;
                    let revision_id = FolderRevisionId::new(decode_field(fields[1])?)?;
                    let entry = FolderEntry::new(
                        pack_string_to_path(&decode_field(fields[2])?),
                        FileVersionId::new(decode_field(fields[3])?)?,
                    )?;
                    revision_entries.push((revision_id, entry));
                }
                Some("checkpoint") => {
                    expect_fields(&fields, 5, line_index + 2)?;
                    checkpoints.push(Checkpoint::new(
                        CheckpointId::new(decode_field(fields[1])?)?,
                        FolderRevisionId::new(decode_field(fields[2])?)?,
                        decode_field(fields[3])?,
                        decode_field(fields[4])?,
                    )?);
                }
                Some("pin") => {
                    expect_fields(&fields, 5, line_index + 2)?;
                    pins.push(Pin::new(
                        PinId::new(decode_field(fields[1])?)?,
                        FolderRevisionId::new(decode_field(fields[2])?)?,
                        decode_field(fields[3])?,
                        decode_field(fields[4])?,
                    )?);
                }
                Some("object") => {
                    expect_fields(&fields, 5, line_index + 2)?;
                    let object_id = ObjectId::from_blake3_hex(decode_field(fields[1])?)?;
                    let size_bytes = fields[2].parse::<u64>().map_err(|_| {
                        PackError::InvalidFormat(format!(
                            "line {} has invalid object size",
                            line_index + 2
                        ))
                    })?;
                    let compression = compression_from_pack(fields[3]).ok_or_else(|| {
                        PackError::InvalidFormat(format!(
                            "line {} has unknown compression",
                            line_index + 2
                        ))
                    })?;
                    let availability = match availability_from_pack(fields[4]) {
                        Some(availability) => availability,
                        None => {
                            let payload = hex_decode(fields[4])?;
                            object_payloads.push(PackObjectPayload {
                                object_id: object_id.clone(),
                                compression: compression.clone(),
                                payload,
                            });
                            PackPayloadAvailability::Inline
                        }
                    };
                    objects.push(PackObject {
                        object_id,
                        size_bytes,
                        compression,
                        availability,
                    });
                }
                Some("object-payload") => {
                    expect_fields(&fields, 4, line_index + 2)?;
                    object_payloads.push(PackObjectPayload {
                        object_id: ObjectId::from_blake3_hex(decode_field(fields[1])?)?,
                        compression: compression_from_pack(fields[2]).ok_or_else(|| {
                            PackError::InvalidFormat(format!(
                                "line {} has unknown compression",
                                line_index + 2
                            ))
                        })?,
                        payload: hex_decode(fields[3])?,
                    });
                }
                Some(tag) => {
                    return Err(PackError::InvalidFormat(format!(
                        "line {} has unknown row tag {tag}",
                        line_index + 2
                    )));
                }
                None => {}
            }
        }

        let shared_folder_id = shared_folder_id
            .ok_or_else(|| PackError::InvalidFormat("pack is missing shared row".to_string()))?;
        let display_name = display_name
            .ok_or_else(|| PackError::InvalidFormat("pack is missing shared row".to_string()))?;
        let latest_revision_id = latest_revision_id
            .ok_or_else(|| PackError::InvalidFormat("pack is missing latest row".to_string()))?;
        let mut revisions = Vec::new();
        for header in revision_headers {
            let mut entries = revision_entries
                .iter()
                .filter(|(revision_id, _)| revision_id == &header.id)
                .map(|(_, entry)| entry.clone())
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| {
                path_to_pack_string(left.path()).cmp(&path_to_pack_string(right.path()))
            });
            revisions.push(FolderRevision::new(
                header.id,
                shared_folder_id.clone(),
                header.parent_id,
                header.boundary,
                entries,
                header.created_at,
            )?);
        }

        Self::new(
            shared_folder_id,
            display_name,
            latest_revision_id,
            file_versions,
            revisions,
            checkpoints,
            pins,
            objects,
            object_payloads,
        )
    }
}

#[derive(Debug)]
pub enum PackError {
    InvalidFormat(String),
    Loom(LoomError),
}

impl fmt::Display for PackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidFormat(message) => write!(f, "invalid Loom pack: {message}"),
            Self::Loom(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PackError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidFormat(_) => None,
            Self::Loom(error) => Some(error),
        }
    }
}

impl From<LoomError> for PackError {
    fn from(error: LoomError) -> Self {
        Self::Loom(error)
    }
}

#[derive(Debug, Clone)]
struct RevisionHeader {
    id: FolderRevisionId,
    parent_id: Option<FolderRevisionId>,
    boundary: RevisionBoundary,
    created_at: String,
}

fn expect_fields(fields: &[&str], expected: usize, line_number: usize) -> Result<(), PackError> {
    if fields.len() != expected {
        return Err(PackError::InvalidFormat(format!(
            "line {line_number} has {} fields, expected {expected}",
            fields.len()
        )));
    }

    Ok(())
}

fn validate_object_payloads(
    objects: &[PackObject],
    payloads: &[PackObjectPayload],
) -> Result<(), PackError> {
    for object in objects {
        let payload = payloads
            .iter()
            .find(|payload| payload.object_id == object.object_id);
        match (object.availability, payload) {
            (PackPayloadAvailability::Inline, Some(payload)) => {
                if payload.payload.len() as u64 != object.size_bytes {
                    return Err(PackError::InvalidFormat(format!(
                        "object {} payload size does not match manifest",
                        object.object_id
                    )));
                }
                if payload.compression != object.compression {
                    return Err(PackError::InvalidFormat(format!(
                        "object {} payload compression does not match manifest",
                        object.object_id
                    )));
                }
            }
            (PackPayloadAvailability::Inline, None) => {
                return Err(PackError::InvalidFormat(format!(
                    "object {} is marked inline but has no payload row",
                    object.object_id
                )));
            }
            (PackPayloadAvailability::Remote, Some(_)) => {
                return Err(PackError::InvalidFormat(format!(
                    "object {} is marked remote but has inline payload bytes",
                    object.object_id
                )));
            }
            (PackPayloadAvailability::Remote, None) => {}
        }
    }

    for payload in payloads {
        if !objects
            .iter()
            .any(|object| object.object_id == payload.object_id)
        {
            return Err(PackError::InvalidFormat(format!(
                "payload row references unknown object {}",
                payload.object_id
            )));
        }
    }

    Ok(())
}

fn file_kind_to_pack(kind: &FileKind) -> &'static str {
    match kind {
        FileKind::File => "file",
        FileKind::Directory => "directory",
        FileKind::Symlink => "symlink",
        FileKind::Unsupported => "unsupported",
    }
}

fn file_kind_from_pack(value: &str) -> Option<FileKind> {
    match value {
        "file" => Some(FileKind::File),
        "directory" => Some(FileKind::Directory),
        "symlink" => Some(FileKind::Symlink),
        "unsupported" => Some(FileKind::Unsupported),
        _ => None,
    }
}

fn revision_boundary_to_pack(boundary: RevisionBoundary) -> &'static str {
    match boundary {
        RevisionBoundary::DebounceWindow => "debounce-window",
        RevisionBoundary::LoomCommand => "loom-command",
        RevisionBoundary::Sync => "sync",
        RevisionBoundary::Restore => "restore",
        RevisionBoundary::SandboxMerge => "sandbox-merge",
        RevisionBoundary::Checkpoint => "checkpoint",
    }
}

fn revision_boundary_from_pack(value: &str) -> Option<RevisionBoundary> {
    match value {
        "debounce-window" => Some(RevisionBoundary::DebounceWindow),
        "loom-command" => Some(RevisionBoundary::LoomCommand),
        "sync" => Some(RevisionBoundary::Sync),
        "restore" => Some(RevisionBoundary::Restore),
        "sandbox-merge" => Some(RevisionBoundary::SandboxMerge),
        "checkpoint" => Some(RevisionBoundary::Checkpoint),
        _ => None,
    }
}

fn compression_to_pack(compression: PackCompression) -> &'static str {
    match compression {
        PackCompression::None => "none",
    }
}

fn compression_from_pack(value: &str) -> Option<PackCompression> {
    match value {
        "none" => Some(PackCompression::None),
        _ => None,
    }
}

fn availability_to_pack(availability: PackPayloadAvailability) -> &'static str {
    match availability {
        PackPayloadAvailability::Inline => "inline",
        PackPayloadAvailability::Remote => "remote",
    }
}

fn availability_from_pack(value: &str) -> Option<PackPayloadAvailability> {
    match value {
        "inline" => Some(PackPayloadAvailability::Inline),
        "remote" => Some(PackPayloadAvailability::Remote),
        _ => None,
    }
}

fn path_to_pack_string(path: &Path) -> String {
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::CurDir => Some(".".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn pack_string_to_path(value: &str) -> PathBuf {
    if value == "." {
        return PathBuf::new();
    }

    value.split('/').collect()
}

fn encode_field(value: &str) -> String {
    let mut encoded = String::new();
    for character in value.chars() {
        match character {
            '%' => encoded.push_str("%25"),
            '\t' => encoded.push_str("%09"),
            '\n' => encoded.push_str("%0A"),
            '\r' => encoded.push_str("%0D"),
            _ => encoded.push(character),
        }
    }
    encoded
}

fn decode_field(value: &str) -> Result<String, PackError> {
    let mut decoded = String::new();
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return Err(PackError::InvalidFormat(
                    "truncated percent escape".to_string(),
                ));
            }
            let hex = &value[index + 1..index + 3];
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| PackError::InvalidFormat("invalid percent escape".to_string()))?;
            decoded.push(byte as char);
            index += 3;
        } else {
            let character = value[index..]
                .chars()
                .next()
                .expect("index is inside the string");
            decoded.push(character);
            index += character.len_utf8();
        }
    }
    Ok(decoded)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

fn hex_decode(value: &str) -> Result<Vec<u8>, PackError> {
    if value.len() % 2 != 0 {
        return Err(PackError::InvalidFormat(
            "hex payload has odd length".to_string(),
        ));
    }

    let mut bytes = Vec::with_capacity(value.len() / 2);
    let mut index = 0;
    while index < value.len() {
        let byte = u8::from_str_radix(&value[index..index + 2], 16)
            .map_err(|_| PackError::InvalidFormat("invalid hex payload".to_string()))?;
        bytes.push(byte);
        index += 2;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object_id() -> ObjectId {
        ObjectId::from_blake3_hex(
            "a3f35a5b6a1d118e4f9f4c23b77d982c84e4c3f4d53172ac89eacd1d29d98f03",
        )
        .expect("object id")
    }

    #[test]
    fn pack_manifest_counts_objects() {
        let manifest = PackManifest {
            shared_folder_id: SharedFolderId::new("folder-devbox").expect("folder id"),
            display_name: "devbox".to_string(),
            latest_revision_id: FolderRevisionId::new("revision-1").expect("revision id"),
            objects: vec![PackObject {
                object_id: object_id(),
                size_bytes: 12,
                compression: PackCompression::None,
                availability: PackPayloadAvailability::Inline,
            }],
        };

        assert_eq!(manifest.object_count(), 1);
    }

    #[test]
    fn uncompressed_pack_round_trips_folder_state() {
        let shared_folder_id = SharedFolderId::new("folder-devbox").expect("folder id");
        let object_id = object_id();
        let version = FileVersion::new(
            FileVersionId::new("file-version-1").expect("file version id"),
            "README.md",
            FileKind::File,
            Some(object_id.clone()),
            Some(12),
            "unix:1",
        )
        .expect("file version");
        let revision = FolderRevision::new(
            FolderRevisionId::new("revision-1").expect("revision id"),
            shared_folder_id.clone(),
            None,
            RevisionBoundary::Sync,
            vec![FolderEntry::new("README.md", version.id().clone()).expect("entry")],
            "unix:2",
        )
        .expect("revision");
        let pack = LoomPack::new(
            shared_folder_id,
            "devbox",
            revision.id().clone(),
            vec![version],
            vec![revision],
            Vec::new(),
            Vec::new(),
            vec![PackObject {
                object_id: object_id.clone(),
                size_bytes: 12,
                compression: PackCompression::None,
                availability: PackPayloadAvailability::Inline,
            }],
            vec![PackObjectPayload {
                object_id,
                compression: PackCompression::None,
                payload: b"hello world!".to_vec(),
            }],
        )
        .expect("pack");

        let decoded = LoomPack::decode(&pack.encode()).expect("pack decodes");

        assert_eq!(decoded, pack);
        assert_eq!(decoded.manifest.object_count(), 1);
        assert_eq!(decoded.inline_object_count(), 1);
    }

    #[test]
    fn metadata_only_pack_round_trips_object_manifest() {
        let shared_folder_id = SharedFolderId::new("folder-devbox").expect("folder id");
        let object_id = object_id();
        let revision_id = FolderRevisionId::new("revision-1").expect("revision id");
        let pack = LoomPack::new(
            shared_folder_id,
            "devbox",
            revision_id,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![PackObject {
                object_id: object_id.clone(),
                size_bytes: 12,
                compression: PackCompression::None,
                availability: PackPayloadAvailability::Remote,
            }],
            Vec::new(),
        )
        .expect("pack");

        let decoded = LoomPack::decode(&pack.encode()).expect("pack decodes");

        assert_eq!(decoded.manifest.object_count(), 1);
        assert_eq!(decoded.inline_object_count(), 0);
        assert_eq!(decoded.manifest.objects[0].object_id, object_id);
        assert_eq!(
            decoded.manifest.objects[0].availability,
            PackPayloadAvailability::Remote
        );
    }

    #[test]
    fn legacy_inline_object_rows_still_decode() {
        let encoded = format!(
            "loom-pack-v1\nshared\tfolder-devbox\tdevbox\nlatest\trevision-1\nobject\t{}\t12\tnone\t{}\n",
            object_id(),
            hex_encode(b"hello world!")
        );

        let decoded = LoomPack::decode(encoded.as_bytes()).expect("legacy pack decodes");

        assert_eq!(decoded.manifest.object_count(), 1);
        assert_eq!(decoded.inline_object_count(), 1);
        assert_eq!(
            decoded.manifest.objects[0].availability,
            PackPayloadAvailability::Inline
        );
    }
}

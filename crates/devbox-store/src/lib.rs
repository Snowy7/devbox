//! SQLite-backed local metadata boundary for Devbox.

mod blob_cache;

pub use blob_cache::{BlobCache, BlobCacheError, BlobCacheResult, BlobRef};

use devbox_auth::{
    revoke_trusted_device, AuthSession, DeviceProjectCursor, DeviceTrustRecord, KeyEnvelope,
    PairingApproval, PairingInvitation,
};
use devbox_core::{BlobId, DomainIdError, ManifestEntryKind, PolicyDecision, ProjectId};
use rusqlite::{params, Connection, OptionalExtension};
use std::fmt;
use std::path::{Component, Path, PathBuf};

pub const CURRENT_SCHEMA_VERSION: u32 = 6;

const SUMMARY_TABLES: &[&str] = &[
    "projects",
    "snapshots",
    "manifest_entries",
    "blobs",
    "chunks",
    "operations",
    "pending_local_changes",
    "local_accounts",
    "local_devices",
    "auth_sessions",
    "pairing_invitations",
    "trusted_devices",
    "key_envelopes",
    "revocation_markers",
    "device_project_cursors",
    "policies",
    "policy_evaluations",
    "restore_attempts",
];

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    InvalidMigrationState(String),
    InvalidSnapshotDraft(String),
    InvalidStoredValue(String),
    DuplicateSnapshotId(String),
    PairingInvitationAlreadyClaimed(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(error) => write!(f, "{error}"),
            Self::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "SQLite schema version {found} is newer than supported version {supported}"
            ),
            Self::InvalidMigrationState(message) => f.write_str(message),
            Self::InvalidSnapshotDraft(message) => f.write_str(message),
            Self::InvalidStoredValue(message) => f.write_str(message),
            Self::DuplicateSnapshotId(id) => write!(f, "snapshot already exists: {id}"),
            Self::PairingInvitationAlreadyClaimed(id) => {
                write!(f, "pairing invitation is already claimed or missing: {id}")
            }
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::UnsupportedSchemaVersion { .. }
            | Self::InvalidMigrationState(_)
            | Self::InvalidSnapshotDraft(_)
            | Self::InvalidStoredValue(_)
            | Self::DuplicateSnapshotId(_)
            | Self::PairingInvitationAlreadyClaimed(_) => None,
        }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open_in_memory() -> StoreResult<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    pub fn open_file(path: impl AsRef<Path>) -> StoreResult<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(conn: Connection) -> StoreResult<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let enabled: u8 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        if enabled != 1 {
            return Err(StoreError::InvalidMigrationState(
                "SQLite foreign key enforcement is disabled".to_string(),
            ));
        }

        Ok(Self { conn })
    }

    pub fn apply_migrations(&self) -> StoreResult<()> {
        let version = self.schema_version()?;
        if version > CURRENT_SCHEMA_VERSION {
            return Err(StoreError::UnsupportedSchemaVersion {
                found: version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );
            "#,
        )?;

        if version < 1 {
            self.conn.execute_batch(INITIAL_SCHEMA)?;
        }

        if version < 2 {
            self.conn.execute_batch(MIGRATION_2_MANIFEST_UNSUPPORTED)?;
        }

        if version < 3 {
            self.conn.execute_batch(MIGRATION_3_PENDING_LOCAL_CHANGES)?;
        }

        if version < 4 {
            self.conn.execute_batch(MIGRATION_4_LOCAL_IDENTITY)?;
        }

        if version < 5 {
            self.conn.execute_batch(MIGRATION_5_AUTH_DEVICE_PAIRING)?;
        }

        if version < 6 {
            self.conn
                .execute_batch(MIGRATION_6_PAIRING_INVITATION_SINGLE_USE)?;
        }

        Ok(())
    }

    pub fn schema_version(&self) -> StoreResult<u32> {
        let version: u32 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok(version)
    }

    pub fn schema_summary(&self) -> StoreResult<SchemaSummary> {
        let version = self.schema_version()?;
        let tables = SUMMARY_TABLES
            .iter()
            .map(|table| self.table_count(table))
            .collect::<StoreResult<Vec<_>>>()?;

        Ok(SchemaSummary { version, tables })
    }

    fn table_count(&self, table: &str) -> StoreResult<TableCount> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let rows = self.conn.query_row(&sql, [], |row| row.get(0))?;

        Ok(TableCount {
            table: table.to_string(),
            rows,
        })
    }

    pub fn insert_project(&self, project: &NewProject<'_>) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                project.id,
                project.root_path,
                project.kind,
                project.display_name,
                project.discovered_at,
            ],
        )?;

        Ok(())
    }

    pub fn upsert_project(&self, project: &NewProject<'_>) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                root_path = excluded.root_path,
                kind = excluded.kind,
                display_name = excluded.display_name
            "#,
            params![
                project.id,
                project.root_path,
                project.kind,
                project.display_name,
                project.discovered_at,
            ],
        )?;

        Ok(())
    }

    pub fn project(&self, id: &str) -> StoreResult<Option<ProjectRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT id, root_path, kind, display_name, discovered_at
                FROM projects
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok(ProjectRecord {
                        id: row.get(0)?,
                        root_path: row.get(1)?,
                        kind: row.get(2)?,
                        display_name: row.get(3)?,
                        discovered_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_snapshot(&self, snapshot: &NewSnapshot<'_>) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO snapshots (
                id,
                project_id,
                parent_snapshot_id,
                created_at,
                reason,
                manifest_entry_count,
                total_size_bytes
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                snapshot.id,
                snapshot.project_id,
                snapshot.parent_snapshot_id,
                snapshot.created_at,
                snapshot.reason,
                snapshot.manifest_entry_count,
                snapshot.total_size_bytes,
            ],
        )?;

        Ok(())
    }

    pub fn snapshot(&self, id: &str) -> StoreResult<Option<SnapshotRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    project_id,
                    parent_snapshot_id,
                    created_at,
                    reason,
                    manifest_entry_count,
                    total_size_bytes
                FROM snapshots
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok(SnapshotRecord {
                        id: row.get(0)?,
                        project_id: row.get(1)?,
                        parent_snapshot_id: row.get(2)?,
                        created_at: row.get(3)?,
                        reason: row.get(4)?,
                        manifest_entry_count: row.get(5)?,
                        total_size_bytes: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn current_timestamp(&self) -> StoreResult<String> {
        self.conn
            .query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
                row.get(0)
            })
            .map_err(StoreError::from)
    }

    pub fn ensure_local_identity(
        &mut self,
        options: &EnsureLocalIdentityOptions<'_>,
    ) -> StoreResult<LocalIdentityRecord> {
        if let Some(identity) = self.local_identity()? {
            return Ok(identity);
        }

        let created_at = self.current_timestamp()?;
        let account_id = random_prefixed_id("account")?;
        let device_id = random_prefixed_id("device")?;
        let sync_key_hex = random_key_hex()?;
        let device_key_hex = random_key_hex()?;
        let display_name = options
            .device_name
            .filter(|name| !name.trim().is_empty())
            .map(str::trim)
            .unwrap_or("local device");

        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO local_accounts (id, display_name, sync_key_hex, created_at)
            VALUES (?1, 'local account', ?2, ?3)
            "#,
            params![account_id, sync_key_hex, created_at],
        )?;
        tx.execute(
            r#"
            INSERT INTO local_devices (
                id,
                account_id,
                display_name,
                device_key_hex,
                is_local,
                created_at,
                last_seen_at
            )
            VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
            "#,
            params![
                device_id,
                account_id,
                display_name,
                device_key_hex,
                created_at,
            ],
        )?;
        tx.commit()?;

        self.local_identity()?.ok_or_else(|| {
            StoreError::InvalidMigrationState("local identity was not persisted".to_string())
        })
    }

    pub fn local_identity(&self) -> StoreResult<Option<LocalIdentityRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    a.id,
                    d.id,
                    d.display_name,
                    a.created_at,
                    d.created_at,
                    d.last_seen_at,
                    a.sync_key_hex,
                    d.device_key_hex
                FROM local_accounts a
                JOIN local_devices d ON d.account_id = a.id
                WHERE d.is_local = 1
                ORDER BY d.created_at ASC, d.id ASC
                LIMIT 1
                "#,
                [],
                |row| {
                    Ok(LocalIdentityRecord {
                        account_id: row.get(0)?,
                        device_id: row.get(1)?,
                        device_name: row.get(2)?,
                        account_created_at: row.get(3)?,
                        device_created_at: row.get(4)?,
                        last_seen_at: row.get(5)?,
                        sync_key_hex: row.get(6)?,
                        device_key_hex: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_devices(&self) -> StoreResult<Vec<DeviceRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT id, account_id, display_name, is_local, created_at, last_seen_at
            FROM local_devices
            ORDER BY is_local DESC, created_at ASC, id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            let is_local: u8 = row.get(3)?;
            Ok(DeviceRecord {
                id: row.get(0)?,
                account_id: row.get(1)?,
                display_name: row.get(2)?,
                is_local: is_local == 1,
                created_at: row.get(4)?,
                last_seen_at: row.get(5)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn upsert_auth_session(&self, session: &AuthSession) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO auth_sessions (
                account_id,
                provider_kind,
                subject,
                session_state,
                proof_issued_at,
                last_refreshed_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(account_id) DO UPDATE SET
                provider_kind = excluded.provider_kind,
                subject = excluded.subject,
                session_state = excluded.session_state,
                proof_issued_at = excluded.proof_issued_at,
                last_refreshed_at = excluded.last_refreshed_at
            "#,
            params![
                session.account_id,
                session.provider_kind,
                session.subject,
                session.session_state,
                session.proof_issued_at,
                session.last_refreshed_at,
            ],
        )?;

        Ok(())
    }

    pub fn auth_session(&self, account_id: &str) -> StoreResult<Option<AuthSession>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    account_id,
                    provider_kind,
                    subject,
                    session_state,
                    proof_issued_at,
                    last_refreshed_at
                FROM auth_sessions
                WHERE account_id = ?1
                "#,
                params![account_id],
                |row| {
                    Ok(AuthSession {
                        account_id: row.get(0)?,
                        provider_kind: row.get(1)?,
                        subject: row.get(2)?,
                        session_state: row.get(3)?,
                        proof_issued_at: row.get(4)?,
                        last_refreshed_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn insert_pairing_invitation(&self, invitation: &PairingInvitation) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO pairing_invitations (
                id,
                account_id,
                inviter_device_id,
                secret_hash_hex,
                status,
                created_at,
                expires_at_unix,
                approved_device_id
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                invitation.id,
                invitation.account_id,
                invitation.inviter_device_id,
                invitation.secret_hash_hex,
                invitation.status,
                invitation.created_at,
                invitation.expires_at_unix,
                invitation.approved_device_id,
            ],
        )?;

        Ok(())
    }

    pub fn pairing_invitation(&self, id: &str) -> StoreResult<Option<PairingInvitation>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    account_id,
                    inviter_device_id,
                    secret_hash_hex,
                    status,
                    created_at,
                    expires_at_unix,
                    approved_device_id
                FROM pairing_invitations
                WHERE id = ?1
                "#,
                params![id],
                |row| {
                    Ok(PairingInvitation {
                        id: row.get(0)?,
                        account_id: row.get(1)?,
                        inviter_device_id: row.get(2)?,
                        secret_hash_hex: row.get(3)?,
                        status: row.get(4)?,
                        created_at: row.get(5)?,
                        expires_at_unix: row.get(6)?,
                        approved_device_id: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn persist_pairing_approval(&mut self, approval: &PairingApproval) -> StoreResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO local_devices (
                id,
                account_id,
                display_name,
                device_key_hex,
                is_local,
                created_at,
                last_seen_at
            )
            VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)
            "#,
            params![
                approval.device.device_id,
                approval.device.account_id,
                approval.device.display_name,
                approval.device.device_key_hex,
                approval.device.approved_at,
            ],
        )?;
        tx.execute(
            r#"
            INSERT INTO key_envelopes (
                id,
                account_id,
                device_id,
                key_ref,
                ciphertext_hex,
                created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                approval.envelope.id,
                approval.envelope.account_id,
                approval.envelope.device_id,
                approval.envelope.key_ref,
                approval.envelope.ciphertext_hex,
                approval.envelope.created_at,
            ],
        )?;
        let claimed = tx.execute(
            r#"
            UPDATE pairing_invitations
            SET status = 'approved',
                approved_device_id = ?1
            WHERE id = ?2 AND status = 'pending'
            "#,
            params![approval.device.device_id, approval.device.invitation_id],
        )?;
        if claimed != 1 {
            return Err(StoreError::PairingInvitationAlreadyClaimed(
                approval.device.invitation_id.clone(),
            ));
        }
        tx.execute(
            r#"
            INSERT INTO trusted_devices (
                device_id,
                account_id,
                invitation_id,
                trust_state,
                approved_at,
                revoked_at,
                key_envelope_id
            )
            VALUES (?1, ?2, ?3, 'approved', ?4, NULL, ?5)
            "#,
            params![
                approval.device.device_id,
                approval.device.account_id,
                approval.device.invitation_id,
                approval.device.approved_at,
                approval.envelope.id,
            ],
        )?;
        tx.commit()?;

        Ok(())
    }

    pub fn key_envelope_for_device(&self, device_id: &str) -> StoreResult<Option<KeyEnvelope>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    account_id,
                    device_id,
                    key_ref,
                    ciphertext_hex,
                    created_at
                FROM key_envelopes
                WHERE device_id = ?1
                ORDER BY created_at DESC, id ASC
                LIMIT 1
                "#,
                params![device_id],
                |row| {
                    Ok(KeyEnvelope {
                        id: row.get(0)?,
                        account_id: row.get(1)?,
                        device_id: row.get(2)?,
                        key_ref: row.get(3)?,
                        ciphertext_hex: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn list_device_trust(&self) -> StoreResult<Vec<DeviceTrustRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                d.id,
                d.account_id,
                d.display_name,
                d.is_local,
                COALESCE(t.trust_state, CASE WHEN d.is_local = 1 THEN 'current-local' ELSE 'known' END),
                t.approved_at,
                t.revoked_at,
                d.last_seen_at
            FROM local_devices d
            LEFT JOIN trusted_devices t ON t.device_id = d.id
            ORDER BY d.is_local DESC, d.created_at ASC, d.id ASC
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            let is_local: u8 = row.get(3)?;
            Ok(DeviceTrustRecord {
                device_id: row.get(0)?,
                account_id: row.get(1)?,
                display_name: row.get(2)?,
                is_local: is_local == 1,
                trust_state: row.get(4)?,
                approved_at: row.get(5)?,
                revoked_at: row.get(6)?,
                last_seen_at: row.get(7)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn device_trust(&self, device_id: &str) -> StoreResult<Option<DeviceTrustRecord>> {
        Ok(self
            .list_device_trust()?
            .into_iter()
            .find(|device| device.device_id == device_id))
    }

    pub fn revoke_device(
        &mut self,
        device_id: &str,
        reason: Option<&str>,
        revoked_at: &str,
    ) -> StoreResult<DeviceTrustRecord> {
        let device = self.device_trust(device_id)?.ok_or_else(|| {
            StoreError::InvalidStoredValue(format!("device not found: {device_id}"))
        })?;
        if device.is_local {
            return Err(StoreError::InvalidStoredValue(
                "refusing to revoke the current local device in local/mock auth".to_string(),
            ));
        }
        revoke_trusted_device(&device)
            .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;

        let marker_id = random_prefixed_id("revocation")?;
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            UPDATE trusted_devices
            SET trust_state = 'revoked',
                revoked_at = ?1
            WHERE device_id = ?2
            "#,
            params![revoked_at, device_id],
        )?;
        tx.execute(
            r#"
            INSERT INTO revocation_markers (
                id,
                account_id,
                device_id,
                revoked_at,
                reason
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![marker_id, device.account_id, device_id, revoked_at, reason,],
        )?;
        tx.commit()?;

        self.device_trust(device_id)?.ok_or_else(|| {
            StoreError::InvalidMigrationState("revoked device disappeared".to_string())
        })
    }

    pub fn upsert_device_project_cursor(&self, cursor: &DeviceProjectCursor) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO device_project_cursors (
                account_id,
                device_id,
                project_id,
                cursor_value,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(account_id, device_id, project_id) DO UPDATE SET
                cursor_value = excluded.cursor_value,
                updated_at = excluded.updated_at
            "#,
            params![
                cursor.account_id,
                cursor.device_id,
                cursor.project_id,
                cursor.cursor_value,
                cursor.updated_at,
            ],
        )?;

        Ok(())
    }

    pub fn device_project_cursor(
        &self,
        account_id: &str,
        device_id: &str,
        project_id: &str,
    ) -> StoreResult<Option<DeviceProjectCursor>> {
        self.conn
            .query_row(
                r#"
                SELECT account_id, device_id, project_id, cursor_value, updated_at
                FROM device_project_cursors
                WHERE account_id = ?1 AND device_id = ?2 AND project_id = ?3
                "#,
                params![account_id, device_id, project_id],
                |row| {
                    Ok(DeviceProjectCursor {
                        account_id: row.get(0)?,
                        device_id: row.get(1)?,
                        project_id: row.get(2)?,
                        cursor_value: row.get(3)?,
                        updated_at: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn persist_draft_snapshot(
        &mut self,
        draft: &NewSnapshotDraft<'_>,
    ) -> StoreResult<PersistedSnapshot> {
        if self.snapshot(draft.snapshot.id)?.is_some() {
            return Err(StoreError::DuplicateSnapshotId(
                draft.snapshot.id.to_string(),
            ));
        }

        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                root_path = excluded.root_path,
                kind = excluded.kind,
                display_name = excluded.display_name
            "#,
            params![
                draft.project.id,
                draft.project.root_path,
                draft.project.kind,
                draft.project.display_name,
                draft.project.discovered_at,
            ],
        )?;

        tx.execute(
            r#"
            INSERT INTO snapshots (
                id,
                project_id,
                parent_snapshot_id,
                created_at,
                reason,
                manifest_entry_count,
                total_size_bytes
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                draft.snapshot.id,
                draft.snapshot.project_id,
                draft.snapshot.parent_snapshot_id,
                draft.snapshot.created_at,
                draft.snapshot.reason,
                draft.snapshot.manifest_entry_count,
                draft.snapshot.total_size_bytes,
            ],
        )?;

        for (index, entry) in draft.entries.iter().enumerate() {
            let path = path_to_store_string(entry.relative_path);
            let (decision, reason) = policy_to_store(entry.policy_decision);

            if let Some(blob_id) = entry.blob_id {
                let object_ref = entry.object_ref.ok_or_else(|| {
                    StoreError::InvalidSnapshotDraft(format!(
                        "manifest entry {path} has blob id {blob_id} without an object ref"
                    ))
                })?;

                tx.execute(
                    r#"
                    INSERT INTO blobs (id, hash_algorithm, size_bytes, object_ref, created_at)
                    VALUES (?1, 'blake3', ?2, ?3, ?4)
                    ON CONFLICT(id) DO UPDATE SET
                        size_bytes = excluded.size_bytes,
                        object_ref = excluded.object_ref
                    "#,
                    params![
                        blob_id.as_str(),
                        entry.size_bytes,
                        object_ref,
                        draft.snapshot.created_at,
                    ],
                )?;
            }

            tx.execute(
                r#"
                INSERT INTO manifest_entries (
                    snapshot_id,
                    path,
                    entry_kind,
                    blob_id,
                    target_path,
                    file_mode,
                    size_bytes,
                    policy_decision,
                    policy_reason
                )
                VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, ?6, ?7)
                "#,
                params![
                    draft.snapshot.id,
                    path,
                    kind_to_store(&entry.kind),
                    entry.blob_id.map(BlobId::as_str),
                    entry.size_bytes,
                    decision,
                    reason,
                ],
            )?;

            if !matches!(entry.policy_decision, PolicyDecision::Include) {
                tx.execute(
                    r#"
                    INSERT INTO policy_evaluations (
                        id,
                        policy_id,
                        project_id,
                        snapshot_id,
                        path,
                        decision,
                        reason,
                        evaluated_at
                    )
                    VALUES (?1, NULL, ?2, ?3, ?4, ?5, ?6, ?7)
                    "#,
                    params![
                        format!("policy-eval-{}-{index}", draft.snapshot.id),
                        draft.snapshot.project_id,
                        draft.snapshot.id,
                        path,
                        decision,
                        reason,
                        draft.snapshot.created_at,
                    ],
                )?;
            }
        }

        tx.commit()?;
        self.snapshot_with_entries(draft.snapshot.id)?
            .ok_or_else(|| {
                StoreError::InvalidMigrationState("persisted snapshot is missing".into())
            })
    }

    pub fn list_snapshots(&self) -> StoreResult<Vec<SnapshotListRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                s.id,
                s.project_id,
                p.root_path,
                p.display_name,
                s.created_at,
                s.manifest_entry_count,
                s.total_size_bytes
            FROM snapshots s
            JOIN projects p ON p.id = s.project_id
            ORDER BY s.created_at DESC, s.id ASC
            "#,
        )?;

        let rows = statement.query_map([], |row| {
            Ok(SnapshotListRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                project_root_path: row.get(2)?,
                project_display_name: row.get(3)?,
                created_at: row.get(4)?,
                manifest_entry_count: row.get(5)?,
                total_size_bytes: row.get(6)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    pub fn snapshot_with_entries(&self, id: &str) -> StoreResult<Option<PersistedSnapshot>> {
        let Some(snapshot) = self.snapshot(id)? else {
            return Ok(None);
        };
        let project = self.project(&snapshot.project_id)?.ok_or_else(|| {
            StoreError::InvalidStoredValue(format!(
                "snapshot {} references missing project {}",
                snapshot.id, snapshot.project_id
            ))
        })?;
        let entries = self.manifest_entries(id)?;

        Ok(Some(PersistedSnapshot {
            project,
            snapshot,
            entries,
        }))
    }

    pub fn latest_snapshot_for_project(
        &self,
        project_id: &str,
    ) -> StoreResult<Option<PersistedSnapshot>> {
        let snapshot_id = self
            .conn
            .query_row(
                r#"
                SELECT id
                FROM snapshots
                WHERE project_id = ?1
                ORDER BY created_at DESC, id ASC
                LIMIT 1
                "#,
                params![project_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        match snapshot_id {
            Some(snapshot_id) => self.snapshot_with_entries(&snapshot_id),
            None => Ok(None),
        }
    }

    pub fn replace_pending_local_changes(
        &mut self,
        project: &NewProject<'_>,
        changes: &[NewPendingLocalChange<'_>],
        detected_at: &str,
    ) -> StoreResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT INTO projects (id, root_path, kind, display_name, discovered_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                root_path = excluded.root_path,
                kind = excluded.kind,
                display_name = excluded.display_name
            "#,
            params![
                project.id,
                project.root_path,
                project.kind,
                project.display_name,
                project.discovered_at,
            ],
        )?;

        tx.execute(
            "DELETE FROM pending_local_changes WHERE project_id = ?1",
            params![project.id],
        )?;

        for change in changes {
            if let (Some(blob_id), Some(object_ref)) = (change.blob_id, change.object_ref) {
                tx.execute(
                    r#"
                    INSERT INTO blobs (id, hash_algorithm, size_bytes, object_ref, created_at)
                    VALUES (?1, 'blake3', ?2, ?3, ?4)
                    ON CONFLICT(id) DO UPDATE SET
                        size_bytes = excluded.size_bytes,
                        object_ref = excluded.object_ref
                    "#,
                    params![blob_id.as_str(), change.size_bytes, object_ref, detected_at],
                )?;
            }

            tx.execute(
                r#"
                INSERT INTO pending_local_changes (
                    id,
                    project_id,
                    base_snapshot_id,
                    path,
                    change_kind,
                    entry_kind,
                    previous_blob_id,
                    blob_id,
                    object_ref,
                    size_bytes,
                    status,
                    detected_at
                )
                VALUES (?1, ?2, ?3, ?4, ?5, 'file', ?6, ?7, ?8, ?9, 'pending', ?10)
                "#,
                params![
                    change.id,
                    project.id,
                    change.base_snapshot_id,
                    path_to_store_string(change.relative_path),
                    change.kind.as_str(),
                    change.previous_blob_id.map(BlobId::as_str),
                    change.blob_id.map(BlobId::as_str),
                    change.object_ref,
                    change.size_bytes,
                    detected_at,
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    pub fn list_pending_local_changes(
        &self,
        project_id: Option<&str>,
    ) -> StoreResult<Vec<PendingLocalChangeRecord>> {
        let mut statement = if project_id.is_some() {
            self.conn.prepare(
                r#"
                SELECT
                    id,
                    project_id,
                    base_snapshot_id,
                    path,
                    change_kind,
                    entry_kind,
                    previous_blob_id,
                    blob_id,
                    object_ref,
                    size_bytes,
                    status,
                    detected_at
                FROM pending_local_changes
                WHERE project_id = ?1
                ORDER BY project_id ASC, path ASC, change_kind ASC, id ASC
                "#,
            )?
        } else {
            self.conn.prepare(
                r#"
                SELECT
                    id,
                    project_id,
                    base_snapshot_id,
                    path,
                    change_kind,
                    entry_kind,
                    previous_blob_id,
                    blob_id,
                    object_ref,
                    size_bytes,
                    status,
                    detected_at
                FROM pending_local_changes
                ORDER BY project_id ASC, path ASC, change_kind ASC, id ASC
                "#,
            )?
        };

        let map_row = |row: &rusqlite::Row<'_>| {
            Ok(RawPendingLocalChangeRecord {
                id: row.get(0)?,
                project_id: row.get(1)?,
                base_snapshot_id: row.get(2)?,
                relative_path: PathBuf::from(row.get::<_, String>(3)?),
                change_kind: row.get(4)?,
                entry_kind: row.get(5)?,
                previous_blob_id: row.get(6)?,
                blob_id: row.get(7)?,
                object_ref: row.get(8)?,
                size_bytes: row.get(9)?,
                status: row.get(10)?,
                detected_at: row.get(11)?,
            })
        };

        let rows = match project_id {
            Some(project_id) => statement
                .query_map(params![project_id], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
            None => statement
                .query_map([], map_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };

        rows.into_iter()
            .map(PendingLocalChangeRecord::try_from)
            .collect()
    }

    pub fn clear_pending_local_changes(&self, project_id: Option<&str>) -> StoreResult<usize> {
        let changed = match project_id {
            Some(project_id) => self.conn.execute(
                "DELETE FROM pending_local_changes WHERE project_id = ?1",
                params![project_id],
            )?,
            None => self.conn.execute("DELETE FROM pending_local_changes", [])?,
        };

        Ok(changed)
    }

    fn manifest_entries(&self, snapshot_id: &str) -> StoreResult<Vec<ManifestEntryRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                me.path,
                me.entry_kind,
                me.blob_id,
                b.object_ref,
                me.size_bytes,
                me.policy_decision,
                me.policy_reason
            FROM manifest_entries me
            LEFT JOIN blobs b ON b.id = me.blob_id
            WHERE me.snapshot_id = ?1
            ORDER BY me.id ASC
            "#,
        )?;

        let rows = statement.query_map(params![snapshot_id], |row| {
            Ok(RawManifestEntryRecord {
                relative_path: PathBuf::from(row.get::<_, String>(0)?),
                kind: row.get(1)?,
                blob_id: row.get(2)?,
                object_ref: row.get(3)?,
                size_bytes: row.get(4)?,
                policy_decision: row.get(5)?,
                policy_reason: row.get(6)?,
            })
        })?;

        rows.map(|row| {
            let record = row?;
            let blob_id = record
                .blob_id
                .map(BlobId::from_blake3_hex)
                .transpose()
                .map_err(|error| invalid_domain_value("blob id", error))?;

            Ok(ManifestEntryRecord {
                relative_path: record.relative_path,
                kind: kind_from_store(&record.kind)?,
                size_bytes: record.size_bytes,
                blob_id,
                object_ref: record.object_ref,
                policy_decision: policy_from_store(record.policy_decision, record.policy_reason)?,
            })
        })
        .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSummary {
    pub version: u32,
    pub tables: Vec<TableCount>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableCount {
    pub table: String,
    pub rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EnsureLocalIdentityOptions<'a> {
    pub device_name: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIdentityRecord {
    pub account_id: String,
    pub device_id: String,
    pub device_name: String,
    pub account_created_at: String,
    pub device_created_at: String,
    pub last_seen_at: String,
    pub sync_key_hex: String,
    pub device_key_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRecord {
    pub id: String,
    pub account_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub created_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProject<'a> {
    pub id: &'a str,
    pub root_path: &'a str,
    pub kind: &'a str,
    pub display_name: &'a str,
    pub discovered_at: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub root_path: String,
    pub kind: String,
    pub display_name: String,
    pub discovered_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshot<'a> {
    pub id: &'a str,
    pub project_id: &'a str,
    pub parent_snapshot_id: Option<&'a str>,
    pub created_at: &'a str,
    pub reason: &'a str,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRecord {
    pub id: String,
    pub project_id: String,
    pub parent_snapshot_id: Option<String>,
    pub created_at: String,
    pub reason: String,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshotDraft<'a> {
    pub project: NewProject<'a>,
    pub snapshot: NewSnapshot<'a>,
    pub entries: Vec<NewSnapshotManifestEntry<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSnapshotManifestEntry<'a> {
    pub relative_path: &'a Path,
    pub kind: ManifestEntryKind,
    pub size_bytes: u64,
    pub blob_id: Option<&'a BlobId>,
    pub object_ref: Option<&'a str>,
    pub policy_decision: &'a PolicyDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotListRecord {
    pub id: String,
    pub project_id: String,
    pub project_root_path: String,
    pub project_display_name: String,
    pub created_at: String,
    pub manifest_entry_count: u64,
    pub total_size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSnapshot {
    pub project: ProjectRecord,
    pub snapshot: SnapshotRecord,
    pub entries: Vec<ManifestEntryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestEntryRecord {
    pub relative_path: PathBuf,
    pub kind: ManifestEntryKind,
    pub size_bytes: u64,
    pub blob_id: Option<BlobId>,
    pub object_ref: Option<String>,
    pub policy_decision: PolicyDecision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalChangeKind {
    Created,
    Modified,
    Deleted,
}

impl LocalChangeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPendingLocalChange<'a> {
    pub id: &'a str,
    pub base_snapshot_id: Option<&'a str>,
    pub relative_path: &'a Path,
    pub kind: LocalChangeKind,
    pub previous_blob_id: Option<&'a BlobId>,
    pub blob_id: Option<&'a BlobId>,
    pub object_ref: Option<&'a str>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLocalChangeRecord {
    pub id: String,
    pub project_id: String,
    pub base_snapshot_id: Option<String>,
    pub relative_path: PathBuf,
    pub change_kind: LocalChangeKind,
    pub entry_kind: ManifestEntryKind,
    pub previous_blob_id: Option<BlobId>,
    pub blob_id: Option<BlobId>,
    pub object_ref: Option<String>,
    pub size_bytes: u64,
    pub status: String,
    pub detected_at: String,
}

#[derive(Debug)]
struct RawManifestEntryRecord {
    relative_path: PathBuf,
    kind: String,
    size_bytes: u64,
    blob_id: Option<String>,
    object_ref: Option<String>,
    policy_decision: String,
    policy_reason: Option<String>,
}

#[derive(Debug)]
struct RawPendingLocalChangeRecord {
    id: String,
    project_id: String,
    base_snapshot_id: Option<String>,
    relative_path: PathBuf,
    change_kind: String,
    entry_kind: String,
    previous_blob_id: Option<String>,
    blob_id: Option<String>,
    object_ref: Option<String>,
    size_bytes: u64,
    status: String,
    detected_at: String,
}

impl TryFrom<RawPendingLocalChangeRecord> for PendingLocalChangeRecord {
    type Error = StoreError;

    fn try_from(record: RawPendingLocalChangeRecord) -> StoreResult<Self> {
        let previous_blob_id = record
            .previous_blob_id
            .map(BlobId::from_blake3_hex)
            .transpose()
            .map_err(|error| invalid_domain_value("previous blob id", error))?;
        let blob_id = record
            .blob_id
            .map(BlobId::from_blake3_hex)
            .transpose()
            .map_err(|error| invalid_domain_value("blob id", error))?;

        Ok(Self {
            id: record.id,
            project_id: record.project_id,
            base_snapshot_id: record.base_snapshot_id,
            relative_path: record.relative_path,
            change_kind: change_kind_from_store(&record.change_kind)?,
            entry_kind: kind_from_store(&record.entry_kind)?,
            previous_blob_id,
            blob_id,
            object_ref: record.object_ref,
            size_bytes: record.size_bytes,
            status: record.status,
            detected_at: record.detected_at,
        })
    }
}

pub fn local_project_id(root_path: impl AsRef<Path>) -> ProjectId {
    let path = root_path.as_ref();
    let identity = path_to_store_string(path);
    let digest = blake3::hash(identity.as_bytes()).to_hex().to_string();
    ProjectId::new(format!("project-local-b3-{digest}"))
        .expect("local project ids are generated from a non-empty prefix")
}

pub fn path_to_store_string(path: &Path) -> String {
    let parts = path
        .components()
        .map(|component| match component {
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
            Component::RootDir => String::new(),
            Component::Normal(part) => part.to_string_lossy().into_owned(),
            Component::CurDir => ".".to_string(),
            Component::ParentDir => "..".to_string(),
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

fn random_prefixed_id(prefix: &str) -> StoreResult<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        StoreError::InvalidMigrationState(format!("failed to generate local identity id: {error}"))
    })?;
    Ok(format!("{prefix}-{}", hex_encode(&bytes)))
}

fn random_key_hex() -> StoreResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        StoreError::InvalidMigrationState(format!(
            "failed to generate local identity key material: {error}"
        ))
    })?;
    Ok(hex_encode(&bytes))
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

fn kind_to_store(kind: &ManifestEntryKind) -> &'static str {
    match kind {
        ManifestEntryKind::File => "file",
        ManifestEntryKind::Directory => "directory",
        ManifestEntryKind::Symlink => "symlink",
        ManifestEntryKind::Unsupported => "unsupported",
    }
}

fn kind_from_store(value: &str) -> StoreResult<ManifestEntryKind> {
    match value {
        "file" => Ok(ManifestEntryKind::File),
        "directory" => Ok(ManifestEntryKind::Directory),
        "symlink" => Ok(ManifestEntryKind::Symlink),
        "unsupported" => Ok(ManifestEntryKind::Unsupported),
        _ => Err(StoreError::InvalidStoredValue(format!(
            "unknown manifest entry kind: {value}"
        ))),
    }
}

fn change_kind_from_store(value: &str) -> StoreResult<LocalChangeKind> {
    match value {
        "created" => Ok(LocalChangeKind::Created),
        "modified" => Ok(LocalChangeKind::Modified),
        "deleted" => Ok(LocalChangeKind::Deleted),
        _ => Err(StoreError::InvalidStoredValue(format!(
            "unknown local change kind: {value}"
        ))),
    }
}

fn policy_to_store(policy: &PolicyDecision) -> (&'static str, Option<&str>) {
    match policy {
        PolicyDecision::Include => ("include", None),
        PolicyDecision::Exclude { reason } => ("exclude", Some(reason.as_str())),
        PolicyDecision::RequiresUserDecision { reason } => {
            ("requires_user_decision", Some(reason.as_str()))
        }
    }
}

fn policy_from_store(decision: String, reason: Option<String>) -> StoreResult<PolicyDecision> {
    match decision.as_str() {
        "include" => Ok(PolicyDecision::Include),
        "exclude" => Ok(PolicyDecision::Exclude {
            reason: reason.unwrap_or_default(),
        }),
        "requires_user_decision" => Ok(PolicyDecision::RequiresUserDecision {
            reason: reason.unwrap_or_default(),
        }),
        _ => Err(StoreError::InvalidStoredValue(format!(
            "unknown policy decision: {decision}"
        ))),
    }
}

fn invalid_domain_value(field: &str, error: DomainIdError) -> StoreError {
    StoreError::InvalidStoredValue(format!("invalid stored {field}: {error}"))
}

const INITIAL_SCHEMA: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    root_path TEXT NOT NULL,
    kind TEXT NOT NULL,
    display_name TEXT NOT NULL,
    discovered_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    parent_snapshot_id TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
    created_at TEXT NOT NULL,
    reason TEXT NOT NULL,
    manifest_entry_count INTEGER NOT NULL DEFAULT 0 CHECK (manifest_entry_count >= 0),
    total_size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (total_size_bytes >= 0)
);

CREATE TABLE IF NOT EXISTS blobs (
    id TEXT PRIMARY KEY,
    hash_algorithm TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    object_ref TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    blob_id TEXT NOT NULL REFERENCES blobs(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL CHECK (chunk_index >= 0),
    chunk_hash TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    object_ref TEXT NOT NULL,
    UNIQUE (blob_id, chunk_index)
);

CREATE TABLE IF NOT EXISTS manifest_entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'directory', 'symlink')),
    blob_id TEXT REFERENCES blobs(id) ON DELETE RESTRICT,
    target_path TEXT,
    file_mode INTEGER,
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    policy_decision TEXT NOT NULL CHECK (
        policy_decision IN ('include', 'exclude', 'requires_user_decision')
    ),
    policy_reason TEXT,
    UNIQUE (snapshot_id, path)
);

CREATE TABLE IF NOT EXISTS operations (
    id TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
    operation_kind TEXT NOT NULL,
    status TEXT NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    details_json TEXT
);

CREATE TABLE IF NOT EXISTS policies (
    id TEXT PRIMARY KEY,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    policy_kind TEXT NOT NULL,
    body_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS policy_evaluations (
    id TEXT PRIMARY KEY,
    policy_id TEXT REFERENCES policies(id) ON DELETE SET NULL,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id TEXT REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('include', 'exclude', 'requires_user_decision')),
    reason TEXT,
    evaluated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS restore_attempts (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE RESTRICT,
    status TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    completed_at TEXT,
    target_path TEXT NOT NULL,
    safety_report_json TEXT,
    error_message TEXT
);

CREATE INDEX IF NOT EXISTS idx_snapshots_project_created
    ON snapshots(project_id, created_at);
CREATE INDEX IF NOT EXISTS idx_manifest_entries_snapshot_path
    ON manifest_entries(snapshot_id, path);
CREATE INDEX IF NOT EXISTS idx_chunks_blob_index
    ON chunks(blob_id, chunk_index);
CREATE INDEX IF NOT EXISTS idx_operations_project_started
    ON operations(project_id, started_at);
CREATE INDEX IF NOT EXISTS idx_policy_evaluations_project_path
    ON policy_evaluations(project_id, path);
CREATE INDEX IF NOT EXISTS idx_restore_attempts_project_requested
    ON restore_attempts(project_id, requested_at);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (1, 'phase_0_metadata_foundation');

PRAGMA user_version = 1;

COMMIT;
"#;

const MIGRATION_2_MANIFEST_UNSUPPORTED: &str = r#"
BEGIN;

CREATE TABLE manifest_entries_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'directory', 'symlink', 'unsupported')),
    blob_id TEXT REFERENCES blobs(id) ON DELETE RESTRICT,
    target_path TEXT,
    file_mode INTEGER,
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    policy_decision TEXT NOT NULL CHECK (
        policy_decision IN ('include', 'exclude', 'requires_user_decision')
    ),
    policy_reason TEXT,
    UNIQUE (snapshot_id, path)
);

INSERT INTO manifest_entries_v2 (
    id,
    snapshot_id,
    path,
    entry_kind,
    blob_id,
    target_path,
    file_mode,
    size_bytes,
    policy_decision,
    policy_reason
)
SELECT
    id,
    snapshot_id,
    path,
    entry_kind,
    blob_id,
    target_path,
    file_mode,
    size_bytes,
    policy_decision,
    policy_reason
FROM manifest_entries;

DROP TABLE manifest_entries;
ALTER TABLE manifest_entries_v2 RENAME TO manifest_entries;

CREATE INDEX IF NOT EXISTS idx_manifest_entries_snapshot_path
    ON manifest_entries(snapshot_id, path);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (2, 'manifest_entries_support_unsupported_kind');

PRAGMA user_version = 2;

COMMIT;
"#;

const MIGRATION_3_PENDING_LOCAL_CHANGES: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS pending_local_changes (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    base_snapshot_id TEXT REFERENCES snapshots(id) ON DELETE SET NULL,
    path TEXT NOT NULL,
    change_kind TEXT NOT NULL CHECK (change_kind IN ('created', 'modified', 'deleted')),
    entry_kind TEXT NOT NULL CHECK (entry_kind = 'file'),
    previous_blob_id TEXT REFERENCES blobs(id) ON DELETE SET NULL,
    blob_id TEXT REFERENCES blobs(id) ON DELETE RESTRICT,
    object_ref TEXT,
    size_bytes INTEGER NOT NULL DEFAULT 0 CHECK (size_bytes >= 0),
    status TEXT NOT NULL CHECK (status = 'pending'),
    detected_at TEXT NOT NULL,
    CHECK (
        (
            change_kind IN ('created', 'modified')
            AND blob_id IS NOT NULL
            AND object_ref IS NOT NULL
        )
        OR (
            change_kind = 'deleted'
            AND blob_id IS NULL
            AND object_ref IS NULL
        )
    ),
    UNIQUE (project_id, path, status)
);

CREATE INDEX IF NOT EXISTS idx_pending_local_changes_project_path
    ON pending_local_changes(project_id, path);
CREATE INDEX IF NOT EXISTS idx_pending_local_changes_base_snapshot
    ON pending_local_changes(base_snapshot_id);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (3, 'pending_local_change_feed');

PRAGMA user_version = 3;

COMMIT;
"#;

const MIGRATION_4_LOCAL_IDENTITY: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS local_accounts (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    sync_key_hex TEXT NOT NULL CHECK (length(sync_key_hex) = 64),
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS local_devices (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    device_key_hex TEXT NOT NULL CHECK (length(device_key_hex) = 64),
    is_local INTEGER NOT NULL CHECK (is_local IN (0, 1)),
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_local_devices_one_local
    ON local_devices(is_local)
    WHERE is_local = 1;
CREATE INDEX IF NOT EXISTS idx_local_devices_account
    ON local_devices(account_id);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (4, 'local_account_device_identity');

PRAGMA user_version = 4;

COMMIT;
"#;

const MIGRATION_5_AUTH_DEVICE_PAIRING: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS auth_sessions (
    account_id TEXT PRIMARY KEY REFERENCES local_accounts(id) ON DELETE CASCADE,
    provider_kind TEXT NOT NULL CHECK (provider_kind IN ('local-mock')),
    subject TEXT NOT NULL,
    session_state TEXT NOT NULL CHECK (session_state IN ('active')),
    proof_issued_at TEXT NOT NULL,
    last_refreshed_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pairing_invitations (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    inviter_device_id TEXT NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    secret_hash_hex TEXT NOT NULL CHECK (length(secret_hash_hex) = 64),
    status TEXT NOT NULL CHECK (status IN ('pending', 'approved')),
    created_at TEXT NOT NULL,
    expires_at_unix INTEGER NOT NULL CHECK (expires_at_unix > 0),
    approved_device_id TEXT REFERENCES local_devices(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS key_envelopes (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    key_ref TEXT NOT NULL,
    ciphertext_hex TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (device_id, key_ref)
);

CREATE TABLE IF NOT EXISTS trusted_devices (
    device_id TEXT PRIMARY KEY REFERENCES local_devices(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    invitation_id TEXT NOT NULL REFERENCES pairing_invitations(id) ON DELETE RESTRICT,
    trust_state TEXT NOT NULL CHECK (trust_state IN ('approved', 'revoked')),
    approved_at TEXT NOT NULL,
    revoked_at TEXT,
    key_envelope_id TEXT NOT NULL REFERENCES key_envelopes(id) ON DELETE RESTRICT
);

CREATE TABLE IF NOT EXISTS revocation_markers (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    revoked_at TEXT NOT NULL,
    reason TEXT
);

CREATE TABLE IF NOT EXISTS device_project_cursors (
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL,
    cursor_value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (account_id, device_id, project_id)
);

CREATE INDEX IF NOT EXISTS idx_pairing_invitations_account_status
    ON pairing_invitations(account_id, status);
CREATE INDEX IF NOT EXISTS idx_key_envelopes_account_device
    ON key_envelopes(account_id, device_id);
CREATE INDEX IF NOT EXISTS idx_trusted_devices_account_state
    ON trusted_devices(account_id, trust_state);
CREATE INDEX IF NOT EXISTS idx_revocation_markers_account_device
    ON revocation_markers(account_id, device_id);
CREATE INDEX IF NOT EXISTS idx_device_project_cursors_device_project
    ON device_project_cursors(device_id, project_id);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (5, 'auth_device_pairing_foundation');

PRAGMA user_version = 5;

COMMIT;
"#;

const MIGRATION_6_PAIRING_INVITATION_SINGLE_USE: &str = r#"
BEGIN;

CREATE UNIQUE INDEX IF NOT EXISTS idx_trusted_devices_invitation_unique
    ON trusted_devices(invitation_id);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (6, 'pairing_invitation_single_use');

PRAGMA user_version = 6;

COMMIT;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_migration_creates_initial_schema() {
        let store = Store::open_in_memory().expect("store opens");

        assert_eq!(store.schema_version().expect("version reads"), 0);

        store.apply_migrations().expect("migrations apply");
        let summary = store.schema_summary().expect("summary reads");

        assert_eq!(summary.version, CURRENT_SCHEMA_VERSION);
        assert_eq!(summary.tables.len(), SUMMARY_TABLES.len());
        assert!(summary.tables.iter().all(|table| table.rows == 0));
    }

    #[test]
    fn migration_is_idempotent() {
        let store = Store::open_in_memory().expect("store opens");

        store.apply_migrations().expect("first migration applies");
        store.apply_migrations().expect("second migration applies");

        let version = store.schema_version().expect("version reads");
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn file_backed_store_persists_schema_version() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("devbox.sqlite3");

        {
            let store = Store::open_file(&path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
        }

        let reopened = Store::open_file(&path).expect("store reopens");
        assert_eq!(
            reopened.schema_version().expect("version reads"),
            CURRENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn local_identity_init_is_idempotent() {
        let mut store = migrated_store();

        let first = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Laptop"),
            })
            .expect("identity initializes");
        let second = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Desktop"),
            })
            .expect("identity reuses existing rows");

        assert_eq!(first, second);
        assert_eq!(first.device_name, "Laptop");
        assert!(first.account_id.starts_with("account-"));
        assert!(first.device_id.starts_with("device-"));
        assert_eq!(first.sync_key_hex.len(), 64);
        assert_eq!(first.device_key_hex.len(), 64);

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "local_accounts"), 1);
        assert_eq!(count(&summary, "local_devices"), 1);
    }

    #[test]
    fn local_devices_list_marks_the_local_device() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Workstation"),
            })
            .expect("identity initializes");

        let devices = store.list_devices().expect("devices list");

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].id, identity.device_id);
        assert_eq!(devices[0].account_id, identity.account_id);
        assert_eq!(devices[0].display_name, "Workstation");
        assert!(devices[0].is_local);
    }

    #[test]
    fn local_device_schema_supports_many_known_devices_for_one_account() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");

        for name in ["Build box", "Travel laptop"] {
            store
                .conn
                .execute(
                    r#"
                    INSERT INTO local_devices (
                        id,
                        account_id,
                        display_name,
                        device_key_hex,
                        is_local,
                        created_at,
                        last_seen_at
                    )
                    VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5)
                    "#,
                    params![
                        format!("device-known-{}", name.replace(' ', "-").to_lowercase()),
                        identity.account_id,
                        name,
                        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                        "2026-06-18T12:00:00Z",
                    ],
                )
                .expect("known device row inserts");
        }

        let devices = store.list_devices().expect("devices list");

        assert_eq!(devices.len(), 3);
        assert_eq!(
            devices
                .iter()
                .filter(|device| device.account_id == identity.account_id)
                .count(),
            3
        );
        assert_eq!(devices.iter().filter(|device| device.is_local).count(), 1);
        assert!(devices
            .iter()
            .any(|device| device.display_name == "Build box" && !device.is_local));
        assert!(devices
            .iter()
            .any(|device| device.display_name == "Travel laptop" && !device.is_local));
    }

    #[test]
    fn mock_auth_session_is_idempotent() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let view = local_identity_view(&identity);
        let first = devbox_auth::mock_login(&view, "2026-06-18T10:00:00Z");
        let second = devbox_auth::mock_login(&view, "2026-06-18T10:01:00Z");

        store
            .upsert_auth_session(&first)
            .expect("first session persists");
        store
            .upsert_auth_session(&second)
            .expect("second session updates");

        let loaded = store
            .auth_session(&identity.account_id)
            .expect("session reads")
            .expect("session exists");
        assert_eq!(loaded.provider_kind, "local-mock");
        assert_eq!(loaded.last_refreshed_at, "2026-06-18T10:01:00Z");
        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "auth_sessions"), 1);
    }

    #[test]
    fn pairing_lifecycle_approves_many_devices_and_creates_envelopes() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let view = local_identity_view(&identity);

        let mut approved_device_ids = Vec::new();
        for (index, name) in ["Laptop", "Build box", "Travel kit"].iter().enumerate() {
            let draft =
                devbox_auth::create_pairing_invitation(&view, "2026-06-18T10:00:00Z", 100, 600)
                    .expect("invitation creates");
            store
                .insert_pairing_invitation(&draft.invitation)
                .expect("invitation persists");
            let loaded = store
                .pairing_invitation(&draft.invitation.id)
                .expect("invitation reads")
                .expect("invitation exists");
            let approval = devbox_auth::approve_pairing_invitation(
                &view,
                &loaded,
                &draft.token,
                name,
                "2026-06-18T10:01:00Z",
                101 + index as i64,
            )
            .expect("approval creates");
            store
                .persist_pairing_approval(&approval)
                .expect("approval persists");

            let envelope = store
                .key_envelope_for_device(&approval.device.device_id)
                .expect("envelope reads")
                .expect("envelope exists");
            let opened = devbox_auth::open_key_envelope(
                &envelope,
                &approval.device.device_key_hex,
                &approval.device.device_id,
            )
            .expect("envelope decrypts");
            assert_eq!(opened, identity.sync_key_hex);
            approved_device_ids.push(approval.device.device_id);
        }

        let trust = store.list_device_trust().expect("trust lists");
        assert_eq!(trust.len(), 4);
        assert_eq!(trust.iter().filter(|device| device.is_local).count(), 1);
        assert_eq!(
            trust
                .iter()
                .filter(|device| device.trust_state == "approved")
                .count(),
            3
        );
        for id in approved_device_ids {
            assert!(trust.iter().any(|device| device.device_id == id));
        }
    }

    #[test]
    fn pairing_invitation_can_only_be_persisted_once() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let view = local_identity_view(&identity);
        let draft = devbox_auth::create_pairing_invitation(&view, "2026-06-18T10:00:00Z", 100, 600)
            .expect("invitation creates");
        store
            .insert_pairing_invitation(&draft.invitation)
            .expect("invitation persists");
        let loaded = store
            .pairing_invitation(&draft.invitation.id)
            .expect("invitation reads")
            .expect("invitation exists");
        let first = devbox_auth::approve_pairing_invitation(
            &view,
            &loaded,
            &draft.token,
            "Laptop",
            "2026-06-18T10:01:00Z",
            101,
        )
        .expect("first approval creates");
        let second = devbox_auth::approve_pairing_invitation(
            &view,
            &loaded,
            &draft.token,
            "Second laptop",
            "2026-06-18T10:01:01Z",
            102,
        )
        .expect("second approval object can be derived from stale pending state");

        store
            .persist_pairing_approval(&first)
            .expect("first approval persists");
        let error = store
            .persist_pairing_approval(&second)
            .expect_err("second approval cannot claim invitation");

        assert!(matches!(
            error,
            StoreError::PairingInvitationAlreadyClaimed(id) if id == draft.invitation.id
        ));
        let trust = store.list_device_trust().expect("trust lists");
        assert_eq!(
            trust
                .iter()
                .filter(|device| device.trust_state == "approved")
                .count(),
            1
        );
        assert!(store
            .device_trust(&second.device.device_id)
            .expect("second device lookup works")
            .is_none());
        assert_eq!(
            store
                .pairing_invitation(&draft.invitation.id)
                .expect("invitation reads")
                .expect("invitation exists")
                .approved_device_id
                .as_deref(),
            Some(first.device.device_id.as_str())
        );
    }

    #[test]
    fn malformed_expired_and_reused_invitations_fail_before_store_update() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let view = local_identity_view(&identity);
        let draft = devbox_auth::create_pairing_invitation(&view, "2026-06-18T10:00:00Z", 100, 1)
            .expect("invitation creates");
        store
            .insert_pairing_invitation(&draft.invitation)
            .expect("invitation persists");
        assert!(devbox_auth::PairingInvitationToken::parse("bad-token").is_err());

        let loaded = store
            .pairing_invitation(&draft.invitation.id)
            .expect("invitation reads")
            .expect("invitation exists");
        let expired = devbox_auth::approve_pairing_invitation(
            &view,
            &loaded,
            &draft.token,
            "Laptop",
            "2026-06-18T10:00:02Z",
            102,
        );
        assert!(matches!(
            expired,
            Err(devbox_auth::AuthError::InvitationExpired { .. })
        ));
        assert_eq!(
            store
                .pairing_invitation(&draft.invitation.id)
                .expect("invitation reads")
                .expect("invitation exists")
                .status,
            "pending"
        );
    }

    #[test]
    fn device_revocation_marks_trust_state_once() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let approval = approve_test_device(&mut store, &identity, "Laptop");

        let revoked = store
            .revoke_device(
                &approval.device.device_id,
                Some("manual test"),
                "2026-06-18T10:02:00Z",
            )
            .expect("device revokes");
        assert_eq!(revoked.trust_state, "revoked");
        assert_eq!(revoked.revoked_at.as_deref(), Some("2026-06-18T10:02:00Z"));

        let second = store.revoke_device(
            &approval.device.device_id,
            Some("again"),
            "2026-06-18T10:03:00Z",
        );
        assert!(matches!(second, Err(StoreError::InvalidStoredValue(_))));
        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "revocation_markers"), 1);
    }

    #[test]
    fn device_project_cursor_upserts_and_reads() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let first = DeviceProjectCursor {
            account_id: identity.account_id.clone(),
            device_id: identity.device_id.clone(),
            project_id: "project-1".to_string(),
            cursor_value: "snapshot-a".to_string(),
            updated_at: "2026-06-18T10:00:00Z".to_string(),
        };
        let second = DeviceProjectCursor {
            cursor_value: "snapshot-b".to_string(),
            updated_at: "2026-06-18T10:01:00Z".to_string(),
            ..first.clone()
        };

        store
            .upsert_device_project_cursor(&first)
            .expect("first cursor persists");
        store
            .upsert_device_project_cursor(&second)
            .expect("second cursor updates");
        let loaded = store
            .device_project_cursor(&identity.account_id, &identity.device_id, "project-1")
            .expect("cursor reads")
            .expect("cursor exists");

        assert_eq!(loaded.cursor_value, "snapshot-b");
        assert_eq!(loaded.updated_at, "2026-06-18T10:01:00Z");
        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "device_project_cursors"), 1);
    }

    #[test]
    fn project_and_snapshot_metadata_round_trip() {
        let store = migrated_store();

        store
            .insert_project(&NewProject {
                id: "project-1",
                root_path: "/workspace/devbox",
                kind: "rust",
                display_name: "devbox",
                discovered_at: "2026-06-18T10:00:00Z",
            })
            .expect("project inserts");

        store
            .insert_snapshot(&NewSnapshot {
                id: "snapshot-1",
                project_id: "project-1",
                parent_snapshot_id: None,
                created_at: "2026-06-18T10:01:00Z",
                reason: "manual",
                manifest_entry_count: 3,
                total_size_bytes: 42,
            })
            .expect("snapshot inserts");

        let project = store
            .project("project-1")
            .expect("project query works")
            .expect("project exists");
        assert_eq!(project.root_path, "/workspace/devbox");
        assert_eq!(project.kind, "rust");

        let snapshot = store
            .snapshot("snapshot-1")
            .expect("snapshot query works")
            .expect("snapshot exists");
        assert_eq!(snapshot.project_id, "project-1");
        assert_eq!(snapshot.manifest_entry_count, 3);
        assert_eq!(snapshot.total_size_bytes, 42);

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "projects"), 1);
        assert_eq!(count(&summary, "snapshots"), 1);
    }

    #[test]
    fn draft_snapshot_persistence_round_trips_manifest_metadata() {
        let mut store = migrated_store();
        let blob_id = blob_id_for(b"hello");
        let include = PolicyDecision::Include;
        let src_path = Path::new("src");
        let file_path = Path::new("src/main.rs");
        let entries = vec![
            NewSnapshotManifestEntry {
                relative_path: src_path,
                kind: ManifestEntryKind::Directory,
                size_bytes: 0,
                blob_id: None,
                object_ref: None,
                policy_decision: &include,
            },
            NewSnapshotManifestEntry {
                relative_path: file_path,
                kind: ManifestEntryKind::File,
                size_bytes: 5,
                blob_id: Some(&blob_id),
                object_ref: Some("blobs/b3/ea/8f/object"),
                policy_decision: &include,
            },
        ];

        store
            .persist_draft_snapshot(&draft("snapshot-1", &entries))
            .expect("draft persists");

        let persisted = store
            .snapshot_with_entries("snapshot-1")
            .expect("snapshot loads")
            .expect("snapshot exists");

        assert_eq!(persisted.project.root_path, "/workspace/devbox");
        assert_eq!(persisted.snapshot.manifest_entry_count, 2);
        assert_eq!(persisted.snapshot.total_size_bytes, 5);
        assert_eq!(
            persisted
                .entries
                .iter()
                .map(|entry| entry.relative_path.clone())
                .collect::<Vec<_>>(),
            vec![PathBuf::from("src"), PathBuf::from("src/main.rs")]
        );
        assert_eq!(persisted.entries[1].blob_id, Some(blob_id));
        assert_eq!(
            persisted.entries[1].object_ref.as_deref(),
            Some("blobs/b3/ea/8f/object")
        );
    }

    #[test]
    fn policy_exclusions_and_deferred_entries_round_trip() {
        let mut store = migrated_store();
        let excluded = PolicyDecision::Exclude {
            reason: "generated dependency directory".to_string(),
        };
        let deferred = PolicyDecision::RequiresUserDecision {
            reason: "symlink capture is deferred until restore safety rules exist".to_string(),
        };
        let node_modules_path = Path::new("node_modules");
        let symlink_path = Path::new("linked.txt");
        let entries = vec![
            NewSnapshotManifestEntry {
                relative_path: node_modules_path,
                kind: ManifestEntryKind::Directory,
                size_bytes: 0,
                blob_id: None,
                object_ref: None,
                policy_decision: &excluded,
            },
            NewSnapshotManifestEntry {
                relative_path: symlink_path,
                kind: ManifestEntryKind::Symlink,
                size_bytes: 0,
                blob_id: None,
                object_ref: None,
                policy_decision: &deferred,
            },
        ];

        store
            .persist_draft_snapshot(&draft("snapshot-policy", &entries))
            .expect("draft persists");

        let persisted = store
            .snapshot_with_entries("snapshot-policy")
            .expect("snapshot loads")
            .expect("snapshot exists");

        assert_eq!(persisted.entries[0].policy_decision, excluded);
        assert_eq!(persisted.entries[1].policy_decision, deferred);

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "policy_evaluations"), 2);
    }

    #[test]
    fn duplicate_snapshot_id_is_reported_clearly() {
        let mut store = migrated_store();
        let include = PolicyDecision::Include;
        let readme_path = Path::new("README.md");
        let entries = vec![NewSnapshotManifestEntry {
            relative_path: readme_path,
            kind: ManifestEntryKind::File,
            size_bytes: 0,
            blob_id: None,
            object_ref: None,
            policy_decision: &include,
        }];

        store
            .persist_draft_snapshot(&draft("snapshot-duplicate", &entries))
            .expect("first draft persists");
        let error = store
            .persist_draft_snapshot(&draft("snapshot-duplicate", &entries))
            .expect_err("duplicate snapshot id fails");

        assert_eq!(
            error.to_string(),
            "snapshot already exists: snapshot-duplicate"
        );
    }

    #[test]
    fn pending_local_change_feed_round_trips_and_replaces_project_rows() {
        let mut store = migrated_store();
        let created_blob = blob_id_for(b"created");
        let modified_previous_blob = blob_id_for(b"before");
        let modified_blob = blob_id_for(b"after");
        let deleted_blob = blob_id_for(b"deleted");
        let created_path = Path::new("created.txt");
        let modified_path = Path::new("src/main.rs");
        let deleted_path = Path::new("deleted.txt");
        let include = PolicyDecision::Include;
        let base_entries = vec![
            NewSnapshotManifestEntry {
                relative_path: modified_path,
                kind: ManifestEntryKind::File,
                size_bytes: 6,
                blob_id: Some(&modified_previous_blob),
                object_ref: Some("blobs/b3/before"),
                policy_decision: &include,
            },
            NewSnapshotManifestEntry {
                relative_path: deleted_path,
                kind: ManifestEntryKind::File,
                size_bytes: 7,
                blob_id: Some(&deleted_blob),
                object_ref: Some("blobs/b3/deleted"),
                policy_decision: &include,
            },
        ];
        store
            .persist_draft_snapshot(&draft("snapshot-base", &base_entries))
            .expect("base snapshot persists");

        let changes = vec![
            NewPendingLocalChange {
                id: "change-created",
                base_snapshot_id: Some("snapshot-base"),
                relative_path: created_path,
                kind: LocalChangeKind::Created,
                previous_blob_id: None,
                blob_id: Some(&created_blob),
                object_ref: Some("blobs/b3/created"),
                size_bytes: 7,
            },
            NewPendingLocalChange {
                id: "change-modified",
                base_snapshot_id: Some("snapshot-base"),
                relative_path: modified_path,
                kind: LocalChangeKind::Modified,
                previous_blob_id: Some(&modified_previous_blob),
                blob_id: Some(&modified_blob),
                object_ref: Some("blobs/b3/after"),
                size_bytes: 5,
            },
            NewPendingLocalChange {
                id: "change-deleted",
                base_snapshot_id: Some("snapshot-base"),
                relative_path: deleted_path,
                kind: LocalChangeKind::Deleted,
                previous_blob_id: Some(&deleted_blob),
                blob_id: None,
                object_ref: None,
                size_bytes: 7,
            },
        ];

        store
            .replace_pending_local_changes(
                &NewProject {
                    id: "project-1",
                    root_path: "/workspace/devbox",
                    kind: "Rust",
                    display_name: "devbox",
                    discovered_at: "2026-06-18T10:02:00Z",
                },
                &changes,
                "2026-06-18T10:03:00Z",
            )
            .expect("changes persist");
        store
            .replace_pending_local_changes(
                &NewProject {
                    id: "project-1",
                    root_path: "/workspace/devbox",
                    kind: "Rust",
                    display_name: "devbox",
                    discovered_at: "2026-06-18T10:02:00Z",
                },
                &changes,
                "2026-06-18T10:04:00Z",
            )
            .expect("repeated change scan replaces existing rows");

        let pending = store
            .list_pending_local_changes(Some("project-1"))
            .expect("pending changes list");
        assert_eq!(pending.len(), 3);
        assert_eq!(
            pending
                .iter()
                .map(|change| (&change.relative_path, &change.change_kind))
                .collect::<Vec<_>>(),
            vec![
                (&PathBuf::from("created.txt"), &LocalChangeKind::Created),
                (&PathBuf::from("deleted.txt"), &LocalChangeKind::Deleted),
                (&PathBuf::from("src/main.rs"), &LocalChangeKind::Modified),
            ]
        );
        assert_eq!(pending[0].blob_id, Some(created_blob));
        assert_eq!(pending[1].blob_id, None);
        assert_eq!(pending[2].previous_blob_id, Some(modified_previous_blob));

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "pending_local_changes"), 3);
    }

    #[test]
    fn pending_local_change_constraints_reject_upload_without_blob() {
        let store = migrated_store();
        store
            .insert_project(&NewProject {
                id: "project-1",
                root_path: "/workspace/devbox",
                kind: "Rust",
                display_name: "devbox",
                discovered_at: "2026-06-18T10:00:00Z",
            })
            .expect("project inserts");

        let result = store.conn.execute(
            r#"
            INSERT INTO pending_local_changes (
                id,
                project_id,
                path,
                change_kind,
                entry_kind,
                size_bytes,
                status,
                detected_at
            )
            VALUES (
                'invalid-change',
                'project-1',
                'missing-blob.txt',
                'created',
                'file',
                10,
                'pending',
                '2026-06-18T10:00:00Z'
            )
            "#,
            [],
        );

        assert!(matches!(
            result,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let store = migrated_store();

        let result = store.insert_snapshot(&NewSnapshot {
            id: "snapshot-without-project",
            project_id: "missing-project",
            parent_snapshot_id: None,
            created_at: "2026-06-18T10:01:00Z",
            reason: "manual",
            manifest_entry_count: 0,
            total_size_bytes: 0,
        });

        assert!(matches!(
            result,
            Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                error,
                _
            ))) if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
    }

    fn migrated_store() -> Store {
        let store = Store::open_in_memory().expect("store opens");
        store.apply_migrations().expect("migrations apply");
        store
    }

    fn draft<'a>(
        snapshot_id: &'a str,
        entries: &'a [NewSnapshotManifestEntry<'a>],
    ) -> NewSnapshotDraft<'a> {
        NewSnapshotDraft {
            project: NewProject {
                id: "project-1",
                root_path: "/workspace/devbox",
                kind: "Rust",
                display_name: "devbox",
                discovered_at: "2026-06-18T10:00:00Z",
            },
            snapshot: NewSnapshot {
                id: snapshot_id,
                project_id: "project-1",
                parent_snapshot_id: None,
                created_at: "2026-06-18T10:01:00Z",
                reason: "manual",
                manifest_entry_count: entries.len() as u64,
                total_size_bytes: entries.iter().map(|entry| entry.size_bytes).sum(),
            },
            entries: entries.to_vec(),
        }
    }

    fn blob_id_for(content: &[u8]) -> BlobId {
        BlobId::from_blake3_hex(blake3::hash(content).to_hex().to_string())
            .expect("BLAKE3 returns valid blob ids")
    }

    fn local_identity_view(identity: &LocalIdentityRecord) -> devbox_auth::LocalIdentityView {
        devbox_auth::LocalIdentityView {
            account_id: identity.account_id.clone(),
            device_id: identity.device_id.clone(),
            device_name: identity.device_name.clone(),
            sync_key_hex: identity.sync_key_hex.clone(),
        }
    }

    fn approve_test_device(
        store: &mut Store,
        identity: &LocalIdentityRecord,
        name: &str,
    ) -> PairingApproval {
        let view = local_identity_view(identity);
        let draft = devbox_auth::create_pairing_invitation(&view, "2026-06-18T10:00:00Z", 100, 600)
            .expect("invitation creates");
        store
            .insert_pairing_invitation(&draft.invitation)
            .expect("invitation persists");
        let approval = devbox_auth::approve_pairing_invitation(
            &view,
            &draft.invitation,
            &draft.token,
            name,
            "2026-06-18T10:01:00Z",
            101,
        )
        .expect("approval creates");
        store
            .persist_pairing_approval(&approval)
            .expect("approval persists");
        approval
    }

    fn count(summary: &SchemaSummary, table: &str) -> u64 {
        summary
            .tables
            .iter()
            .find(|entry| entry.table == table)
            .expect("table is present")
            .rows
    }
}

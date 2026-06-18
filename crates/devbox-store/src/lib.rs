//! SQLite-backed local metadata boundary for Devbox.

mod blob_cache;

pub use blob_cache::{BlobCache, BlobCacheError, BlobCacheResult, BlobRef};

use devbox_auth::{
    complete_device_rotation_intent, consume_recovery_grant as auth_consume_recovery_grant,
    create_key_envelope, hash_session_token_hex, now_unix_seconds,
    revoke_device_rotation_intent as auth_revoke_device_rotation_intent,
    revoke_recovery_grant as auth_revoke_recovery_grant, revoke_trusted_device,
    validate_account_ownership_proof, AccountOwnershipProof, AccountSession, AuthSession,
    DeviceProjectCursor, DeviceRotationIntent, DeviceTrustRecord, KeyEnvelope, PairingApproval,
    PairingInvitation, RecoveryGrant,
};
use devbox_conflict::{ConflictSummary, PathComparisonState};
use devbox_core::{BlobId, DomainIdError, ManifestEntryKind, PolicyDecision, ProjectId};
use rusqlite::{params, Connection, OptionalExtension};
use std::fmt;
use std::path::{Component, Path, PathBuf};

pub const CURRENT_SCHEMA_VERSION: u32 = 9;

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
    "account_ownership_proofs",
    "account_sessions",
    "pairing_invitations",
    "trusted_devices",
    "key_envelopes",
    "recovery_grants",
    "device_rotation_intents",
    "revocation_markers",
    "device_project_cursors",
    "conflicts",
    "conflict_rows",
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
    InvalidConflictStatus(String),
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
            Self::InvalidConflictStatus(status) => write!(f, "invalid conflict status: {status}"),
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
            | Self::InvalidConflictStatus(_)
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

        if version < 7 {
            self.conn
                .execute_batch(MIGRATION_7_CONFLICT_DIVERGENT_SNAPSHOTS)?;
        }

        if version < 8 {
            self.conn
                .execute_batch(MIGRATION_8_PRODUCTION_ACCOUNT_AUTH_BOUNDARY)?;
        }

        if version < 9 {
            self.conn
                .execute_batch(MIGRATION_9_PRODUCTION_PAIRING_RECOVERY_ROTATION)?;
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

    pub fn upsert_account_ownership_proof(&self, proof: &AccountOwnershipProof) -> StoreResult<()> {
        validate_account_ownership_proof(proof, now_unix_seconds())
            .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;
        self.conn.execute(
            r#"
            INSERT INTO account_ownership_proofs (
                account_id,
                provider_kind,
                provider_issuer,
                provider_subject,
                verified_email,
                verified_domain,
                proof_state,
                proof_issued_at,
                proof_expires_at_unix,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?8, ?8)
            ON CONFLICT(account_id) DO UPDATE SET
                provider_kind = excluded.provider_kind,
                provider_issuer = excluded.provider_issuer,
                provider_subject = excluded.provider_subject,
                verified_email = excluded.verified_email,
                verified_domain = excluded.verified_domain,
                proof_state = excluded.proof_state,
                proof_issued_at = excluded.proof_issued_at,
                proof_expires_at_unix = excluded.proof_expires_at_unix,
                updated_at = excluded.updated_at
            "#,
            params![
                proof.account_id,
                proof.provider_kind,
                proof.provider_issuer,
                proof.provider_subject,
                proof.verified_email,
                proof.verified_domain,
                proof.proof_state,
                proof.proof_issued_at,
                proof.proof_expires_at_unix,
            ],
        )?;

        Ok(())
    }

    pub fn account_ownership_proof(
        &self,
        account_id: &str,
    ) -> StoreResult<Option<AccountOwnershipProof>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    account_id,
                    provider_kind,
                    provider_issuer,
                    provider_subject,
                    verified_email,
                    verified_domain,
                    proof_state,
                    proof_issued_at,
                    proof_expires_at_unix
                FROM account_ownership_proofs
                WHERE account_id = ?1
                "#,
                params![account_id],
                account_ownership_proof_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn account_ownership_proof_by_provider(
        &self,
        provider_kind: &str,
        provider_issuer: &str,
        provider_subject: &str,
    ) -> StoreResult<Option<AccountOwnershipProof>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    account_id,
                    provider_kind,
                    provider_issuer,
                    provider_subject,
                    verified_email,
                    verified_domain,
                    proof_state,
                    proof_issued_at,
                    proof_expires_at_unix
                FROM account_ownership_proofs
                WHERE provider_kind = ?1
                    AND provider_issuer = ?2
                    AND provider_subject = ?3
                "#,
                params![provider_kind, provider_issuer, provider_subject],
                account_ownership_proof_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn upsert_account_session(&self, session: &AccountSession) -> StoreResult<()> {
        ensure_session_hash(session)?;
        self.conn.execute(
            r#"
            INSERT INTO account_sessions (
                session_id,
                account_id,
                provider_kind,
                provider_issuer,
                provider_subject,
                session_token_hash_hex,
                session_state,
                created_at,
                expires_at_unix,
                revoked_at,
                last_seen_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(session_id) DO UPDATE SET
                provider_kind = excluded.provider_kind,
                provider_issuer = excluded.provider_issuer,
                provider_subject = excluded.provider_subject,
                session_token_hash_hex = excluded.session_token_hash_hex,
                session_state = excluded.session_state,
                expires_at_unix = excluded.expires_at_unix,
                revoked_at = excluded.revoked_at,
                last_seen_at = excluded.last_seen_at
            "#,
            params![
                session.session_id,
                session.account_id,
                session.provider_kind,
                session.provider_issuer,
                session.provider_subject,
                session.session_token_hash_hex,
                session.session_state,
                session.created_at,
                session.expires_at_unix,
                session.revoked_at,
                session.last_seen_at,
            ],
        )?;

        Ok(())
    }

    pub fn account_session(&self, session_id: &str) -> StoreResult<Option<AccountSession>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    session_id,
                    account_id,
                    provider_kind,
                    provider_issuer,
                    provider_subject,
                    session_token_hash_hex,
                    session_state,
                    created_at,
                    expires_at_unix,
                    revoked_at,
                    last_seen_at
                FROM account_sessions
                WHERE session_id = ?1
                "#,
                params![session_id],
                account_session_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn account_session_for_token(
        &self,
        raw_session_token: &str,
    ) -> StoreResult<Option<AccountSession>> {
        let session_hash = hash_session_token_hex(raw_session_token);
        self.conn
            .query_row(
                r#"
                SELECT
                    session_id,
                    account_id,
                    provider_kind,
                    provider_issuer,
                    provider_subject,
                    session_token_hash_hex,
                    session_state,
                    created_at,
                    expires_at_unix,
                    revoked_at,
                    last_seen_at
                FROM account_sessions
                WHERE session_token_hash_hex = ?1
                "#,
                params![session_hash],
                account_session_from_row,
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
                created_at,
                rotation_generation
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                approval.envelope.id,
                approval.envelope.account_id,
                approval.envelope.device_id,
                approval.envelope.key_ref,
                approval.envelope.ciphertext_hex,
                approval.envelope.created_at,
                approval.envelope.rotation_generation,
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
                    created_at,
                    rotation_generation
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
                        rotation_generation: row.get::<_, i64>(6)? as u64,
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

    pub fn upsert_recovery_grant(&self, grant: &RecoveryGrant) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO recovery_grants (
                id,
                account_id,
                device_id,
                grant_ref,
                status,
                created_at,
                expires_at_unix,
                consumed_at,
                revoked_at,
                audit_label
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                consumed_at = excluded.consumed_at,
                revoked_at = excluded.revoked_at
            "#,
            params![
                grant.id,
                grant.account_id,
                grant.device_id,
                grant.grant_ref,
                grant.status,
                grant.created_at,
                grant.expires_at_unix,
                grant.consumed_at,
                grant.revoked_at,
                grant.audit_label,
            ],
        )?;
        Ok(())
    }

    pub fn recovery_grant(&self, grant_id: &str) -> StoreResult<Option<RecoveryGrant>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    account_id,
                    device_id,
                    grant_ref,
                    status,
                    created_at,
                    expires_at_unix,
                    consumed_at,
                    revoked_at,
                    audit_label
                FROM recovery_grants
                WHERE id = ?1
                "#,
                params![grant_id],
                recovery_grant_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn revoke_recovery_grant(
        &self,
        grant_id: &str,
        revoked_at: &str,
    ) -> StoreResult<RecoveryGrant> {
        let grant = self.recovery_grant(grant_id)?.ok_or_else(|| {
            StoreError::InvalidStoredValue(format!("recovery grant not found: {grant_id}"))
        })?;
        let revoked = auth_revoke_recovery_grant(&grant, revoked_at)
            .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;
        self.upsert_recovery_grant(&revoked)?;
        Ok(revoked)
    }

    pub fn consume_recovery_grant(
        &self,
        grant_id: &str,
        consumed_at: &str,
        now_unix: i64,
    ) -> StoreResult<RecoveryGrant> {
        let grant = self.recovery_grant(grant_id)?.ok_or_else(|| {
            StoreError::InvalidStoredValue(format!("recovery grant not found: {grant_id}"))
        })?;
        let consumed = auth_consume_recovery_grant(&grant, consumed_at, now_unix)
            .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;
        let updated = self.conn.execute(
            r#"
            UPDATE recovery_grants
            SET status = 'consumed',
                consumed_at = ?2
            WHERE id = ?1
                AND status = 'pending'
                AND consumed_at IS NULL
                AND revoked_at IS NULL
                AND expires_at_unix > ?3
            "#,
            params![grant_id, consumed_at, now_unix],
        )?;
        if updated != 1 {
            return Err(StoreError::InvalidStoredValue(
                "recovery grant was not pending for consumption".to_string(),
            ));
        }
        Ok(consumed)
    }

    pub fn upsert_device_rotation_intent(&self, intent: &DeviceRotationIntent) -> StoreResult<()> {
        self.conn.execute(
            r#"
            INSERT INTO device_rotation_intents (
                id,
                account_id,
                device_id,
                requested_by_session_id,
                status,
                reason,
                created_at,
                expires_at_unix,
                completed_at,
                revoked_at,
                key_envelope_generation
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                completed_at = excluded.completed_at,
                revoked_at = excluded.revoked_at,
                key_envelope_generation = excluded.key_envelope_generation
            "#,
            params![
                intent.id,
                intent.account_id,
                intent.device_id,
                intent.requested_by_session_id,
                intent.status,
                intent.reason,
                intent.created_at,
                intent.expires_at_unix,
                intent.completed_at,
                intent.revoked_at,
                intent.key_envelope_generation as i64,
            ],
        )?;
        Ok(())
    }

    pub fn device_rotation_intent(
        &self,
        intent_id: &str,
    ) -> StoreResult<Option<DeviceRotationIntent>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    account_id,
                    device_id,
                    requested_by_session_id,
                    status,
                    reason,
                    created_at,
                    expires_at_unix,
                    completed_at,
                    revoked_at,
                    key_envelope_generation
                FROM device_rotation_intents
                WHERE id = ?1
                "#,
                params![intent_id],
                device_rotation_intent_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn revoke_device_rotation_intent(
        &self,
        intent_id: &str,
        revoked_at: &str,
    ) -> StoreResult<DeviceRotationIntent> {
        let intent = self.device_rotation_intent(intent_id)?.ok_or_else(|| {
            StoreError::InvalidStoredValue(format!("device rotation intent not found: {intent_id}"))
        })?;
        let revoked = auth_revoke_device_rotation_intent(&intent, revoked_at)
            .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;
        self.upsert_device_rotation_intent(&revoked)?;
        Ok(revoked)
    }

    pub fn rotate_key_envelope_for_device(
        &mut self,
        intent: &DeviceRotationIntent,
        sync_key_hex: &str,
        rotated_at: &str,
        now_unix: i64,
    ) -> StoreResult<(DeviceRotationIntent, KeyEnvelope)> {
        let tx = self.conn.transaction()?;
        let stored_intent = tx
            .query_row(
                r#"
                SELECT
                    id,
                    account_id,
                    device_id,
                    requested_by_session_id,
                    status,
                    reason,
                    created_at,
                    expires_at_unix,
                    completed_at,
                    revoked_at,
                    key_envelope_generation
                FROM device_rotation_intents
                WHERE id = ?1
                "#,
                params![intent.id],
                device_rotation_intent_from_row,
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::InvalidStoredValue(format!(
                    "device rotation intent not found: {}",
                    intent.id
                ))
            })?;
        if stored_intent.account_id != intent.account_id
            || stored_intent.device_id != intent.device_id
            || stored_intent.key_envelope_generation != intent.key_envelope_generation
        {
            return Err(StoreError::InvalidStoredValue(
                "device rotation intent does not match stored pending intent".to_string(),
            ));
        }
        if stored_intent.status != "pending" {
            return Err(StoreError::InvalidStoredValue(format!(
                "device rotation intent {} is not pending; current status is {}",
                stored_intent.id, stored_intent.status
            )));
        }

        let (existing, device_key_hex) = tx
            .query_row(
                r#"
                SELECT
                    e.id,
                    e.account_id,
                    e.device_id,
                    e.key_ref,
                    e.ciphertext_hex,
                    e.created_at,
                    e.rotation_generation,
                    d.device_key_hex
                FROM key_envelopes e
                JOIN local_devices d
                    ON d.id = e.device_id
                    AND d.account_id = e.account_id
                WHERE e.account_id = ?1 AND e.device_id = ?2
                ORDER BY e.created_at DESC, e.id ASC
                LIMIT 1
                "#,
                params![stored_intent.account_id, stored_intent.device_id],
                |row| {
                    Ok((
                        KeyEnvelope {
                            id: row.get(0)?,
                            account_id: row.get(1)?,
                            device_id: row.get(2)?,
                            key_ref: row.get(3)?,
                            ciphertext_hex: row.get(4)?,
                            created_at: row.get(5)?,
                            rotation_generation: row.get::<_, i64>(6)? as u64,
                        },
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::InvalidStoredValue(format!(
                    "key envelope not found for device: {}",
                    stored_intent.device_id
                ))
            })?;
        if existing.rotation_generation != stored_intent.key_envelope_generation {
            return Err(StoreError::InvalidStoredValue(
                "device rotation intent generation is stale".to_string(),
            ));
        }

        let mut rotated = create_key_envelope(
            &stored_intent.account_id,
            &stored_intent.device_id,
            &device_key_hex,
            sync_key_hex,
            rotated_at,
        )
        .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;
        rotated.id = existing.id;
        rotated.key_ref = existing.key_ref;
        rotated.rotation_generation = existing.rotation_generation + 1;
        let completed = complete_device_rotation_intent(
            &stored_intent,
            rotated_at,
            now_unix,
            rotated.rotation_generation,
        )
        .map_err(|error| StoreError::InvalidStoredValue(error.to_string()))?;

        let claimed = tx.execute(
            r#"
            UPDATE device_rotation_intents
            SET status = ?1,
                completed_at = ?2,
                key_envelope_generation = ?3
            WHERE id = ?4
                AND account_id = ?5
                AND device_id = ?6
                AND status = 'pending'
                AND key_envelope_generation = ?7
                AND expires_at_unix > ?8
            "#,
            params![
                completed.status,
                completed.completed_at,
                completed.key_envelope_generation as i64,
                completed.id,
                completed.account_id,
                completed.device_id,
                existing.rotation_generation as i64,
                now_unix,
            ],
        )?;
        if claimed != 1 {
            return Err(StoreError::InvalidStoredValue(
                "device rotation intent was not claimed for completion".to_string(),
            ));
        }

        let updated_envelope = tx.execute(
            r#"
            UPDATE key_envelopes
            SET ciphertext_hex = ?1,
                created_at = ?2,
                rotation_generation = ?3
            WHERE id = ?4
                AND account_id = ?5
                AND device_id = ?6
                AND rotation_generation = ?7
            "#,
            params![
                rotated.ciphertext_hex,
                rotated.created_at,
                rotated.rotation_generation as i64,
                rotated.id,
                rotated.account_id,
                rotated.device_id,
                existing.rotation_generation as i64,
            ],
        )?;
        if updated_envelope != 1 {
            return Err(StoreError::InvalidStoredValue(
                "key envelope generation changed before rotation commit".to_string(),
            ));
        }
        tx.commit()?;

        Ok((completed, rotated))
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

    pub fn latest_snapshot_for_project_excluding(
        &self,
        project_id: &str,
        excluded_snapshot_id: &str,
    ) -> StoreResult<Option<PersistedSnapshot>> {
        let snapshot_id = self
            .conn
            .query_row(
                r#"
                SELECT id
                FROM snapshots
                WHERE project_id = ?1 AND id != ?2
                ORDER BY created_at DESC, id ASC
                LIMIT 1
                "#,
                params![project_id, excluded_snapshot_id],
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

    pub fn persist_conflict(
        &mut self,
        conflict: &NewConflict<'_>,
        rows: &[NewConflictRow<'_>],
    ) -> StoreResult<ConflictWithRows> {
        let tx = self.conn.transaction()?;
        tx.execute(
            r#"
            INSERT OR IGNORE INTO conflicts (
                id,
                project_id,
                base_snapshot_id,
                base_snapshot_key,
                local_snapshot_id,
                incoming_snapshot_id,
                status,
                created_at,
                row_count,
                same_count,
                local_only_count,
                incoming_only_count,
                local_deleted_count,
                incoming_deleted_count,
                both_modified_same_count,
                both_modified_different_count,
                policy_excluded_count,
                policy_deferred_count,
                policy_blocked_count,
                unsupported_count
            )
            VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, 'open', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                ?17, ?18, ?19
            )
            "#,
            params![
                conflict.id,
                conflict.project_id,
                conflict.base_snapshot_id,
                conflict.base_snapshot_id.unwrap_or("-"),
                conflict.local_snapshot_id,
                conflict.incoming_snapshot_id,
                conflict.created_at,
                conflict.summary.total() as u64,
                conflict.summary.same() as u64,
                conflict.summary.local_only() as u64,
                conflict.summary.incoming_only() as u64,
                conflict.summary.local_deleted() as u64,
                conflict.summary.incoming_deleted() as u64,
                conflict.summary.both_modified_same() as u64,
                conflict.summary.both_modified_different() as u64,
                conflict.summary.policy_excluded() as u64,
                conflict.summary.policy_deferred() as u64,
                conflict.summary.policy_blocked() as u64,
                conflict.summary.unsupported() as u64,
            ],
        )?;

        for (index, row) in rows.iter().enumerate() {
            tx.execute(
                r#"
                INSERT OR IGNORE INTO conflict_rows (
                    conflict_id,
                    row_index,
                    path,
                    state,
                    entry_kind,
                    base_blob_id,
                    local_blob_id,
                    incoming_blob_id,
                    base_size_bytes,
                    local_size_bytes,
                    incoming_size_bytes,
                    local_policy_decision,
                    local_policy_reason,
                    incoming_policy_decision,
                    incoming_policy_reason
                )
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
                "#,
                params![
                    conflict.id,
                    index as u64,
                    path_to_store_string(row.path),
                    row.state.as_str(),
                    kind_to_store(row.entry_kind),
                    row.base_blob_id.map(BlobId::as_str),
                    row.local_blob_id.map(BlobId::as_str),
                    row.incoming_blob_id.map(BlobId::as_str),
                    row.base_size_bytes,
                    row.local_size_bytes,
                    row.incoming_size_bytes,
                    row.local_policy_decision
                        .map(policy_to_store)
                        .map(|value| value.0),
                    row.local_policy_decision
                        .and_then(|policy| policy_to_store(policy).1),
                    row.incoming_policy_decision
                        .map(policy_to_store)
                        .map(|value| value.0),
                    row.incoming_policy_decision
                        .and_then(|policy| policy_to_store(policy).1),
                ],
            )?;
        }

        tx.commit()?;
        self.conflict_with_rows(conflict.id)?.ok_or_else(|| {
            StoreError::InvalidMigrationState("persisted conflict is missing".into())
        })
    }

    pub fn list_conflicts(&self, project_id: Option<&str>) -> StoreResult<Vec<ConflictRecord>> {
        let mut statement = if project_id.is_some() {
            self.conn.prepare(
                r#"
                SELECT
                    id,
                    project_id,
                    base_snapshot_id,
                    local_snapshot_id,
                    incoming_snapshot_id,
                    status,
                    created_at,
                    updated_at,
                    row_count,
                    same_count,
                    local_only_count,
                    incoming_only_count,
                    local_deleted_count,
                    incoming_deleted_count,
                    both_modified_same_count,
                    both_modified_different_count,
                    policy_excluded_count,
                    policy_deferred_count,
                    policy_blocked_count,
                    unsupported_count
                FROM conflicts
                WHERE project_id = ?1
                ORDER BY created_at DESC, id ASC
                "#,
            )?
        } else {
            self.conn.prepare(
                r#"
                SELECT
                    id,
                    project_id,
                    base_snapshot_id,
                    local_snapshot_id,
                    incoming_snapshot_id,
                    status,
                    created_at,
                    updated_at,
                    row_count,
                    same_count,
                    local_only_count,
                    incoming_only_count,
                    local_deleted_count,
                    incoming_deleted_count,
                    both_modified_same_count,
                    both_modified_different_count,
                    policy_excluded_count,
                    policy_deferred_count,
                    policy_blocked_count,
                    unsupported_count
                FROM conflicts
                ORDER BY created_at DESC, id ASC
                "#,
            )?
        };

        let rows = match project_id {
            Some(project_id) => statement
                .query_map(params![project_id], conflict_record_from_row)?
                .collect::<Result<Vec<_>, _>>()?,
            None => statement
                .query_map([], conflict_record_from_row)?
                .collect::<Result<Vec<_>, _>>()?,
        };

        Ok(rows)
    }

    pub fn conflict_with_rows(&self, id: &str) -> StoreResult<Option<ConflictWithRows>> {
        let Some(conflict) = self.conflict(id)? else {
            return Ok(None);
        };
        let rows = self.conflict_rows(id)?;

        Ok(Some(ConflictWithRows { conflict, rows }))
    }

    pub fn update_conflict_status(
        &self,
        id: &str,
        status: ConflictStatus,
        updated_at: &str,
    ) -> StoreResult<Option<ConflictRecord>> {
        let changed = self.conn.execute(
            r#"
            UPDATE conflicts
            SET status = ?1,
                updated_at = ?2
            WHERE id = ?3
            "#,
            params![status.as_str(), updated_at, id],
        )?;

        if changed == 0 {
            return Ok(None);
        }

        self.conflict(id)
    }

    fn conflict(&self, id: &str) -> StoreResult<Option<ConflictRecord>> {
        self.conn
            .query_row(
                r#"
                SELECT
                    id,
                    project_id,
                    base_snapshot_id,
                    local_snapshot_id,
                    incoming_snapshot_id,
                    status,
                    created_at,
                    updated_at,
                    row_count,
                    same_count,
                    local_only_count,
                    incoming_only_count,
                    local_deleted_count,
                    incoming_deleted_count,
                    both_modified_same_count,
                    both_modified_different_count,
                    policy_excluded_count,
                    policy_deferred_count,
                    policy_blocked_count,
                    unsupported_count
                FROM conflicts
                WHERE id = ?1
                "#,
                params![id],
                conflict_record_from_row,
            )
            .optional()
            .map_err(StoreError::from)
    }

    fn conflict_rows(&self, id: &str) -> StoreResult<Vec<ConflictRowRecord>> {
        let mut statement = self.conn.prepare(
            r#"
            SELECT
                path,
                state,
                entry_kind,
                base_blob_id,
                local_blob_id,
                incoming_blob_id,
                base_size_bytes,
                local_size_bytes,
                incoming_size_bytes,
                local_policy_decision,
                local_policy_reason,
                incoming_policy_decision,
                incoming_policy_reason
            FROM conflict_rows
            WHERE conflict_id = ?1
            ORDER BY row_index ASC, path ASC
            "#,
        )?;

        let rows = statement.query_map(params![id], |row| {
            Ok(RawConflictRowRecord {
                relative_path: PathBuf::from(row.get::<_, String>(0)?),
                state: row.get(1)?,
                entry_kind: row.get(2)?,
                base_blob_id: row.get(3)?,
                local_blob_id: row.get(4)?,
                incoming_blob_id: row.get(5)?,
                base_size_bytes: row.get(6)?,
                local_size_bytes: row.get(7)?,
                incoming_size_bytes: row.get(8)?,
                local_policy_decision: row.get(9)?,
                local_policy_reason: row.get(10)?,
                incoming_policy_decision: row.get(11)?,
                incoming_policy_reason: row.get(12)?,
            })
        })?;

        rows.map(|row| ConflictRowRecord::try_from(row?)).collect()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictStatus {
    Open,
    Resolved,
    Dismissed,
}

impl ConflictStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Dismissed => "dismissed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConflict<'a> {
    pub id: &'a str,
    pub project_id: &'a str,
    pub base_snapshot_id: Option<&'a str>,
    pub local_snapshot_id: &'a str,
    pub incoming_snapshot_id: &'a str,
    pub summary: &'a ConflictSummary,
    pub created_at: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConflictRow<'a> {
    pub path: &'a Path,
    pub state: PathComparisonState,
    pub entry_kind: &'a ManifestEntryKind,
    pub base_blob_id: Option<&'a BlobId>,
    pub local_blob_id: Option<&'a BlobId>,
    pub incoming_blob_id: Option<&'a BlobId>,
    pub base_size_bytes: Option<u64>,
    pub local_size_bytes: Option<u64>,
    pub incoming_size_bytes: Option<u64>,
    pub local_policy_decision: Option<&'a PolicyDecision>,
    pub incoming_policy_decision: Option<&'a PolicyDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictWithRows {
    pub conflict: ConflictRecord,
    pub rows: Vec<ConflictRowRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRecord {
    pub id: String,
    pub project_id: String,
    pub base_snapshot_id: Option<String>,
    pub local_snapshot_id: String,
    pub incoming_snapshot_id: String,
    pub status: ConflictStatus,
    pub created_at: String,
    pub updated_at: String,
    pub row_count: u64,
    pub same_count: u64,
    pub local_only_count: u64,
    pub incoming_only_count: u64,
    pub local_deleted_count: u64,
    pub incoming_deleted_count: u64,
    pub both_modified_same_count: u64,
    pub both_modified_different_count: u64,
    pub policy_excluded_count: u64,
    pub policy_deferred_count: u64,
    pub policy_blocked_count: u64,
    pub unsupported_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictRowRecord {
    pub relative_path: PathBuf,
    pub state: PathComparisonState,
    pub entry_kind: ManifestEntryKind,
    pub base_blob_id: Option<BlobId>,
    pub local_blob_id: Option<BlobId>,
    pub incoming_blob_id: Option<BlobId>,
    pub base_size_bytes: Option<u64>,
    pub local_size_bytes: Option<u64>,
    pub incoming_size_bytes: Option<u64>,
    pub local_policy_decision: Option<PolicyDecision>,
    pub incoming_policy_decision: Option<PolicyDecision>,
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

#[derive(Debug)]
struct RawConflictRowRecord {
    relative_path: PathBuf,
    state: String,
    entry_kind: String,
    base_blob_id: Option<String>,
    local_blob_id: Option<String>,
    incoming_blob_id: Option<String>,
    base_size_bytes: Option<u64>,
    local_size_bytes: Option<u64>,
    incoming_size_bytes: Option<u64>,
    local_policy_decision: Option<String>,
    local_policy_reason: Option<String>,
    incoming_policy_decision: Option<String>,
    incoming_policy_reason: Option<String>,
}

impl TryFrom<RawConflictRowRecord> for ConflictRowRecord {
    type Error = StoreError;

    fn try_from(record: RawConflictRowRecord) -> StoreResult<Self> {
        Ok(Self {
            relative_path: record.relative_path,
            state: comparison_state_from_store(&record.state)?,
            entry_kind: kind_from_store(&record.entry_kind)?,
            base_blob_id: record
                .base_blob_id
                .map(BlobId::from_blake3_hex)
                .transpose()
                .map_err(|error| invalid_domain_value("base blob id", error))?,
            local_blob_id: record
                .local_blob_id
                .map(BlobId::from_blake3_hex)
                .transpose()
                .map_err(|error| invalid_domain_value("local blob id", error))?,
            incoming_blob_id: record
                .incoming_blob_id
                .map(BlobId::from_blake3_hex)
                .transpose()
                .map_err(|error| invalid_domain_value("incoming blob id", error))?,
            base_size_bytes: record.base_size_bytes,
            local_size_bytes: record.local_size_bytes,
            incoming_size_bytes: record.incoming_size_bytes,
            local_policy_decision: optional_policy_from_store(
                record.local_policy_decision,
                record.local_policy_reason,
            )?,
            incoming_policy_decision: optional_policy_from_store(
                record.incoming_policy_decision,
                record.incoming_policy_reason,
            )?,
        })
    }
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

fn account_ownership_proof_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AccountOwnershipProof> {
    Ok(AccountOwnershipProof {
        account_id: row.get(0)?,
        provider_kind: row.get(1)?,
        provider_issuer: row.get(2)?,
        provider_subject: row.get(3)?,
        verified_email: row.get(4)?,
        verified_domain: row.get(5)?,
        proof_state: row.get(6)?,
        proof_issued_at: row.get(7)?,
        proof_expires_at_unix: row.get(8)?,
    })
}

fn account_session_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountSession> {
    Ok(AccountSession {
        session_id: row.get(0)?,
        account_id: row.get(1)?,
        provider_kind: row.get(2)?,
        provider_issuer: row.get(3)?,
        provider_subject: row.get(4)?,
        session_token_hash_hex: row.get(5)?,
        session_state: row.get(6)?,
        created_at: row.get(7)?,
        expires_at_unix: row.get(8)?,
        revoked_at: row.get(9)?,
        last_seen_at: row.get(10)?,
    })
}

fn recovery_grant_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecoveryGrant> {
    Ok(RecoveryGrant {
        id: row.get(0)?,
        account_id: row.get(1)?,
        device_id: row.get(2)?,
        grant_ref: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        expires_at_unix: row.get(6)?,
        consumed_at: row.get(7)?,
        revoked_at: row.get(8)?,
        audit_label: row.get(9)?,
    })
}

fn device_rotation_intent_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<DeviceRotationIntent> {
    Ok(DeviceRotationIntent {
        id: row.get(0)?,
        account_id: row.get(1)?,
        device_id: row.get(2)?,
        requested_by_session_id: row.get(3)?,
        status: row.get(4)?,
        reason: row.get(5)?,
        created_at: row.get(6)?,
        expires_at_unix: row.get(7)?,
        completed_at: row.get(8)?,
        revoked_at: row.get(9)?,
        key_envelope_generation: row.get::<_, i64>(10)? as u64,
    })
}

fn ensure_session_hash(session: &AccountSession) -> StoreResult<()> {
    if session.session_token_hash_hex.len() != 64
        || session
            .session_token_hash_hex
            .as_bytes()
            .iter()
            .any(|byte| !byte.is_ascii_hexdigit())
    {
        return Err(StoreError::InvalidStoredValue(
            "account session token hash must be 64 hex characters".to_string(),
        ));
    }
    Ok(())
}

fn conflict_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConflictRecord> {
    let status: String = row.get(5)?;
    Ok(ConflictRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        base_snapshot_id: row.get(2)?,
        local_snapshot_id: row.get(3)?,
        incoming_snapshot_id: row.get(4)?,
        status: conflict_status_from_store(&status).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        row_count: row.get(8)?,
        same_count: row.get(9)?,
        local_only_count: row.get(10)?,
        incoming_only_count: row.get(11)?,
        local_deleted_count: row.get(12)?,
        incoming_deleted_count: row.get(13)?,
        both_modified_same_count: row.get(14)?,
        both_modified_different_count: row.get(15)?,
        policy_excluded_count: row.get(16)?,
        policy_deferred_count: row.get(17)?,
        policy_blocked_count: row.get(18)?,
        unsupported_count: row.get(19)?,
    })
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

fn conflict_status_from_store(value: &str) -> StoreResult<ConflictStatus> {
    match value {
        "open" => Ok(ConflictStatus::Open),
        "resolved" => Ok(ConflictStatus::Resolved),
        "dismissed" => Ok(ConflictStatus::Dismissed),
        _ => Err(StoreError::InvalidConflictStatus(value.to_string())),
    }
}

fn comparison_state_from_store(value: &str) -> StoreResult<PathComparisonState> {
    match value {
        "same" => Ok(PathComparisonState::Same),
        "local-only" => Ok(PathComparisonState::LocalOnly),
        "incoming-only" => Ok(PathComparisonState::IncomingOnly),
        "local-deleted" => Ok(PathComparisonState::LocalDeleted),
        "incoming-deleted" => Ok(PathComparisonState::IncomingDeleted),
        "both-modified-same" => Ok(PathComparisonState::BothModifiedSame),
        "both-modified-different" => Ok(PathComparisonState::BothModifiedDifferent),
        "policy-excluded" => Ok(PathComparisonState::PolicyExcluded),
        "policy-deferred" => Ok(PathComparisonState::PolicyDeferred),
        "policy-blocked" => Ok(PathComparisonState::PolicyBlocked),
        "unsupported" => Ok(PathComparisonState::Unsupported),
        _ => Err(StoreError::InvalidStoredValue(format!(
            "unknown conflict comparison state: {value}"
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

fn optional_policy_from_store(
    decision: Option<String>,
    reason: Option<String>,
) -> StoreResult<Option<PolicyDecision>> {
    decision
        .map(|decision| policy_from_store(decision, reason))
        .transpose()
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

const MIGRATION_7_CONFLICT_DIVERGENT_SNAPSHOTS: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS conflicts (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    base_snapshot_id TEXT REFERENCES snapshots(id) ON DELETE RESTRICT,
    base_snapshot_key TEXT NOT NULL,
    local_snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE RESTRICT,
    incoming_snapshot_id TEXT NOT NULL REFERENCES snapshots(id) ON DELETE RESTRICT,
    status TEXT NOT NULL CHECK (status IN ('open', 'resolved', 'dismissed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    row_count INTEGER NOT NULL DEFAULT 0 CHECK (row_count >= 0),
    same_count INTEGER NOT NULL DEFAULT 0 CHECK (same_count >= 0),
    local_only_count INTEGER NOT NULL DEFAULT 0 CHECK (local_only_count >= 0),
    incoming_only_count INTEGER NOT NULL DEFAULT 0 CHECK (incoming_only_count >= 0),
    local_deleted_count INTEGER NOT NULL DEFAULT 0 CHECK (local_deleted_count >= 0),
    incoming_deleted_count INTEGER NOT NULL DEFAULT 0 CHECK (incoming_deleted_count >= 0),
    both_modified_same_count INTEGER NOT NULL DEFAULT 0 CHECK (both_modified_same_count >= 0),
    both_modified_different_count INTEGER NOT NULL DEFAULT 0 CHECK (both_modified_different_count >= 0),
    policy_excluded_count INTEGER NOT NULL DEFAULT 0 CHECK (policy_excluded_count >= 0),
    policy_deferred_count INTEGER NOT NULL DEFAULT 0 CHECK (policy_deferred_count >= 0),
    policy_blocked_count INTEGER NOT NULL DEFAULT 0 CHECK (policy_blocked_count >= 0),
    unsupported_count INTEGER NOT NULL DEFAULT 0 CHECK (unsupported_count >= 0),
    UNIQUE (project_id, base_snapshot_key, local_snapshot_id, incoming_snapshot_id)
);

CREATE TABLE IF NOT EXISTS conflict_rows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conflict_id TEXT NOT NULL REFERENCES conflicts(id) ON DELETE CASCADE,
    row_index INTEGER NOT NULL CHECK (row_index >= 0),
    path TEXT NOT NULL,
    state TEXT NOT NULL CHECK (
        state IN (
            'same',
            'local-only',
            'incoming-only',
            'local-deleted',
            'incoming-deleted',
            'both-modified-same',
            'both-modified-different',
            'policy-excluded',
            'policy-deferred',
            'policy-blocked',
            'unsupported'
        )
    ),
    entry_kind TEXT NOT NULL CHECK (entry_kind IN ('file', 'directory', 'symlink', 'unsupported')),
    base_blob_id TEXT REFERENCES blobs(id) ON DELETE SET NULL,
    local_blob_id TEXT REFERENCES blobs(id) ON DELETE SET NULL,
    incoming_blob_id TEXT REFERENCES blobs(id) ON DELETE SET NULL,
    base_size_bytes INTEGER CHECK (base_size_bytes IS NULL OR base_size_bytes >= 0),
    local_size_bytes INTEGER CHECK (local_size_bytes IS NULL OR local_size_bytes >= 0),
    incoming_size_bytes INTEGER CHECK (incoming_size_bytes IS NULL OR incoming_size_bytes >= 0),
    local_policy_decision TEXT CHECK (
        local_policy_decision IS NULL
        OR local_policy_decision IN ('include', 'exclude', 'requires_user_decision')
    ),
    local_policy_reason TEXT,
    incoming_policy_decision TEXT CHECK (
        incoming_policy_decision IS NULL
        OR incoming_policy_decision IN ('include', 'exclude', 'requires_user_decision')
    ),
    incoming_policy_reason TEXT,
    UNIQUE (conflict_id, row_index),
    UNIQUE (conflict_id, path)
);

CREATE INDEX IF NOT EXISTS idx_conflicts_project_status_created
    ON conflicts(project_id, status, created_at);
CREATE INDEX IF NOT EXISTS idx_conflicts_snapshots
    ON conflicts(local_snapshot_id, incoming_snapshot_id);
CREATE INDEX IF NOT EXISTS idx_conflict_rows_conflict_path
    ON conflict_rows(conflict_id, path);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (7, 'conflict_divergent_snapshot_foundation');

PRAGMA user_version = 7;

COMMIT;
"#;

const MIGRATION_8_PRODUCTION_ACCOUNT_AUTH_BOUNDARY: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS account_ownership_proofs (
    account_id TEXT PRIMARY KEY REFERENCES local_accounts(id) ON DELETE CASCADE,
    provider_kind TEXT NOT NULL,
    provider_issuer TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    verified_email TEXT,
    verified_domain TEXT,
    proof_state TEXT NOT NULL CHECK (proof_state = 'verified'),
    proof_issued_at TEXT NOT NULL,
    proof_expires_at_unix INTEGER NOT NULL CHECK (proof_expires_at_unix > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (verified_email IS NOT NULL OR verified_domain IS NOT NULL),
    UNIQUE (account_id, provider_kind, provider_issuer, provider_subject),
    UNIQUE (provider_kind, provider_issuer, provider_subject)
);

CREATE TABLE IF NOT EXISTS account_sessions (
    session_id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    provider_kind TEXT NOT NULL,
    provider_issuer TEXT NOT NULL,
    provider_subject TEXT NOT NULL,
    session_token_hash_hex TEXT NOT NULL CHECK (length(session_token_hash_hex) = 64),
    session_state TEXT NOT NULL CHECK (session_state IN ('active', 'revoked')),
    created_at TEXT NOT NULL,
    expires_at_unix INTEGER NOT NULL CHECK (expires_at_unix > 0),
    revoked_at TEXT,
    last_seen_at TEXT NOT NULL,
    UNIQUE (session_token_hash_hex),
    FOREIGN KEY (account_id, provider_kind, provider_issuer, provider_subject)
        REFERENCES account_ownership_proofs(account_id, provider_kind, provider_issuer, provider_subject)
        ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_account_ownership_proofs_provider_subject
    ON account_ownership_proofs(provider_kind, provider_issuer, provider_subject);
CREATE INDEX IF NOT EXISTS idx_account_sessions_account_state
    ON account_sessions(account_id, session_state, expires_at_unix);
CREATE INDEX IF NOT EXISTS idx_account_sessions_hash
    ON account_sessions(session_token_hash_hex);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (8, 'production_account_auth_boundary');

PRAGMA user_version = 8;

COMMIT;
"#;

const MIGRATION_9_PRODUCTION_PAIRING_RECOVERY_ROTATION: &str = r#"
BEGIN;

ALTER TABLE key_envelopes
    ADD COLUMN rotation_generation INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS recovery_grants (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    grant_ref TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'consumed', 'revoked')),
    created_at TEXT NOT NULL,
    expires_at_unix INTEGER NOT NULL CHECK (expires_at_unix > 0),
    consumed_at TEXT,
    revoked_at TEXT,
    audit_label TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS device_rotation_intents (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES local_accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL REFERENCES local_devices(id) ON DELETE CASCADE,
    requested_by_session_id TEXT REFERENCES account_sessions(session_id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'completed', 'revoked')),
    reason TEXT NOT NULL,
    created_at TEXT NOT NULL,
    expires_at_unix INTEGER NOT NULL CHECK (expires_at_unix > 0),
    completed_at TEXT,
    revoked_at TEXT,
    key_envelope_generation INTEGER NOT NULL CHECK (key_envelope_generation >= 0)
);

CREATE INDEX IF NOT EXISTS idx_recovery_grants_account_device_status
    ON recovery_grants(account_id, device_id, status, expires_at_unix);
CREATE INDEX IF NOT EXISTS idx_device_rotation_intents_account_device_status
    ON device_rotation_intents(account_id, device_id, status, expires_at_unix);

INSERT OR IGNORE INTO schema_migrations (version, name)
VALUES (9, 'production_pairing_recovery_rotation');

PRAGMA user_version = 9;

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
    fn production_account_proof_and_session_round_trip_without_raw_token_persistence() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("devbox.sqlite3");
        let raw_token = "raw-dev-session-token-should-not-persist";
        let provider_subject = "oidc-subject-123";
        let session_id;
        let account_id;

        {
            let mut store = Store::open_file(&db_path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
            let identity = store
                .ensure_local_identity(&EnsureLocalIdentityOptions {
                    device_name: Some("Current machine"),
                })
                .expect("identity initializes");
            account_id = identity.account_id.clone();
            let proof = devbox_auth::create_account_ownership_proof(
                devbox_auth::AccountOwnershipProofInput {
                    account_id: &identity.account_id,
                    provider_kind: "oidc-dev",
                    provider_issuer: "https://issuer.devbox.local",
                    provider_subject,
                    verified_email: Some("user@example.com"),
                    verified_domain: Some("example.com"),
                    proof_issued_at: "2026-06-18T10:00:00Z",
                    proof_expires_at_unix: now_unix_seconds() + 600,
                },
            )
            .expect("proof creates");
            store
                .upsert_account_ownership_proof(&proof)
                .expect("proof persists");
            let session = devbox_auth::create_account_session(
                &proof,
                raw_token,
                "2026-06-18T10:01:00Z",
                101,
                600,
            )
            .expect("session creates");
            session_id = session.session_id.clone();
            store
                .upsert_account_session(&session)
                .expect("session persists");

            let loaded_proof = store
                .account_ownership_proof(&identity.account_id)
                .expect("proof reads")
                .expect("proof exists");
            assert_eq!(
                loaded_proof.verified_email.as_deref(),
                Some("user@example.com")
            );
            let provider_lookup = store
                .account_ownership_proof_by_provider(
                    "oidc-dev",
                    "https://issuer.devbox.local",
                    provider_subject,
                )
                .expect("provider lookup reads")
                .expect("provider lookup exists");
            assert_eq!(provider_lookup.account_id, identity.account_id);

            let loaded_session = store
                .account_session_for_token(raw_token)
                .expect("session lookup by token hash reads")
                .expect("session exists");
            assert_eq!(loaded_session.session_id, session.session_id);
            assert_eq!(loaded_session.session_token_hash_hex.len(), 64);
            assert!(!loaded_session.session_token_hash_hex.contains(raw_token));
            devbox_auth::validate_account_session(&loaded_session, raw_token, 102)
                .expect("session validates");
            let summary = store.schema_summary().expect("summary reads");
            assert_eq!(count(&summary, "account_ownership_proofs"), 1);
            assert_eq!(count(&summary, "account_sessions"), 1);
        }

        let reopened = Store::open_file(&db_path).expect("store reopens");
        reopened.apply_migrations().expect("migrations apply");
        assert_eq!(
            reopened
                .account_session(&session_id)
                .expect("session reads")
                .expect("session exists")
                .account_id,
            account_id
        );
        let persisted_bytes = std::fs::read(&db_path).expect("db bytes read");
        let persisted_text = String::from_utf8_lossy(&persisted_bytes);
        assert!(!persisted_text.contains(raw_token));
    }

    #[test]
    fn production_account_proof_validation_runs_at_local_store_boundary() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let valid =
            devbox_auth::create_account_ownership_proof(devbox_auth::AccountOwnershipProofInput {
                account_id: &identity.account_id,
                provider_kind: "oidc-dev",
                provider_issuer: "https://issuer.devbox.local",
                provider_subject: "oidc-subject-123",
                verified_email: Some("user@example.com"),
                verified_domain: None,
                proof_issued_at: "2026-06-18T10:00:00Z",
                proof_expires_at_unix: now_unix_seconds() + 600,
            })
            .expect("proof creates");

        let mut secret_like = valid.clone();
        secret_like.provider_subject = "provider-secret-should-not-persist".to_string();
        let error = store
            .upsert_account_ownership_proof(&secret_like)
            .expect_err("secret-like provider material is rejected");
        assert!(error
            .to_string()
            .contains("value must not contain secret-looking material"));

        let mut empty_verified_evidence = valid.clone();
        empty_verified_evidence.verified_email = Some("   ".to_string());
        empty_verified_evidence.verified_domain = None;
        let error = store
            .upsert_account_ownership_proof(&empty_verified_evidence)
            .expect_err("empty verified evidence is rejected");
        assert!(error.to_string().contains("value must not be empty"));

        let mut expired = valid;
        expired.proof_expires_at_unix = 1;
        let error = store
            .upsert_account_ownership_proof(&expired)
            .expect_err("expired proof is rejected");
        assert!(error.to_string().contains("ownership proof is expired"));
    }

    #[test]
    fn production_account_session_rejects_raw_64_character_token_as_hash() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let raw_token = format!("raw-token-{}", "x".repeat(54));
        assert_eq!(raw_token.len(), 64);
        let proof =
            devbox_auth::create_account_ownership_proof(devbox_auth::AccountOwnershipProofInput {
                account_id: &identity.account_id,
                provider_kind: "oidc-dev",
                provider_issuer: "https://issuer.devbox.local",
                provider_subject: "oidc-subject-123",
                verified_email: Some("user@example.com"),
                verified_domain: None,
                proof_issued_at: "2026-06-18T10:00:00Z",
                proof_expires_at_unix: now_unix_seconds() + 600,
            })
            .expect("proof creates");
        store
            .upsert_account_ownership_proof(&proof)
            .expect("proof persists");
        let mut session = devbox_auth::create_account_session(
            &proof,
            &raw_token,
            "2026-06-18T10:01:00Z",
            101,
            600,
        )
        .expect("session creates");
        session.session_token_hash_hex = raw_token.clone();

        let error = store
            .upsert_account_session(&session)
            .expect_err("raw token-shaped value is rejected as hash");
        assert_eq!(
            error.to_string(),
            "account session token hash must be 64 hex characters"
        );
        assert!(store
            .account_session(&session.session_id)
            .expect("session lookup works")
            .is_none());
    }

    #[test]
    fn production_account_session_revocation_persists() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let raw_token = "raw-dev-session-token";
        let proof =
            devbox_auth::create_account_ownership_proof(devbox_auth::AccountOwnershipProofInput {
                account_id: &identity.account_id,
                provider_kind: "oidc-dev",
                provider_issuer: "https://issuer.devbox.local",
                provider_subject: "oidc-subject-123",
                verified_email: Some("user@example.com"),
                verified_domain: None,
                proof_issued_at: "2026-06-18T10:00:00Z",
                proof_expires_at_unix: now_unix_seconds() + 600,
            })
            .expect("proof creates");
        store
            .upsert_account_ownership_proof(&proof)
            .expect("proof persists");
        let session = devbox_auth::create_account_session(
            &proof,
            raw_token,
            "2026-06-18T10:01:00Z",
            101,
            600,
        )
        .expect("session creates");
        store
            .upsert_account_session(&session)
            .expect("session persists");
        let revoked = devbox_auth::revoke_account_session(&session, "2026-06-18T10:02:00Z")
            .expect("session revokes");
        store
            .upsert_account_session(&revoked)
            .expect("revoked session persists");

        let loaded = store
            .account_session(&session.session_id)
            .expect("session reads")
            .expect("session exists");
        assert_eq!(loaded.session_state, "revoked");
        assert_eq!(loaded.revoked_at.as_deref(), Some("2026-06-18T10:02:00Z"));
        assert!(matches!(
            devbox_auth::validate_account_session(&loaded, raw_token, 102),
            Err(devbox_auth::AuthError::AccountSessionRevoked { .. })
        ));
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
    fn production_recovery_grant_revocation_is_idempotent_and_sanitized() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("devbox.sqlite3");
        let raw_recovery_secret = "raw-recovery-secret-should-not-persist";

        {
            let mut store = Store::open_file(&db_path).expect("store opens");
            store.apply_migrations().expect("migrations apply");
            let identity = store
                .ensure_local_identity(&EnsureLocalIdentityOptions {
                    device_name: Some("Current machine"),
                })
                .expect("identity initializes");
            let approval = approve_test_device(&mut store, &identity, "Laptop");

            let rejected = devbox_auth::create_recovery_grant(
                &identity.account_id,
                &approval.device.device_id,
                raw_recovery_secret,
                "laptop recovery",
                "2026-06-18T10:00:00Z",
                100,
                600,
            )
            .expect_err("raw recovery secret is rejected");
            assert!(!rejected.to_string().contains(raw_recovery_secret));

            let grant = devbox_auth::create_recovery_grant(
                &identity.account_id,
                &approval.device.device_id,
                "recovery-ref:laptop:alpha",
                "laptop recovery",
                "2026-06-18T10:00:00Z",
                100,
                600,
            )
            .expect("grant creates");
            store.upsert_recovery_grant(&grant).expect("grant persists");
            let first = store
                .revoke_recovery_grant(&grant.id, "2026-06-18T10:01:00Z")
                .expect("grant revokes");
            let second = store
                .revoke_recovery_grant(&grant.id, "2026-06-18T10:02:00Z")
                .expect("grant revokes idempotently");

            assert_eq!(first.revoked_at, second.revoked_at);
            assert_eq!(second.status, "revoked");
            let summary = store.schema_summary().expect("summary reads");
            assert_eq!(count(&summary, "recovery_grants"), 1);

            let consumable = devbox_auth::create_recovery_grant(
                &identity.account_id,
                &approval.device.device_id,
                "recovery-ref:laptop:beta",
                "laptop recovery",
                "2026-06-18T10:03:00Z",
                103,
                600,
            )
            .expect("grant creates");
            store
                .upsert_recovery_grant(&consumable)
                .expect("grant persists");
            let consumed = store
                .consume_recovery_grant(&consumable.id, "2026-06-18T10:04:00Z", 104)
                .expect("grant consumes");
            assert_eq!(consumed.status, "consumed");
            assert_eq!(
                consumed.consumed_at.as_deref(),
                Some("2026-06-18T10:04:00Z")
            );
            let consumed_again =
                store.consume_recovery_grant(&consumable.id, "2026-06-18T10:05:00Z", 105);
            assert!(matches!(
                consumed_again,
                Err(StoreError::InvalidStoredValue(_))
            ));
            let consumed_revoke =
                store.revoke_recovery_grant(&consumable.id, "2026-06-18T10:05:00Z");
            assert!(matches!(
                consumed_revoke,
                Err(StoreError::InvalidStoredValue(_))
            ));
            let loaded_consumed = store
                .recovery_grant(&consumable.id)
                .expect("grant reads")
                .expect("grant exists");
            assert_eq!(
                loaded_consumed.consumed_at.as_deref(),
                Some("2026-06-18T10:04:00Z")
            );
            assert_eq!(loaded_consumed.revoked_at, None);
        }

        let db_bytes = std::fs::read(&db_path).expect("db bytes read");
        let db_text = String::from_utf8_lossy(&db_bytes);
        assert!(!db_text.contains(raw_recovery_secret));
    }

    #[test]
    fn production_device_rotation_intent_bumps_key_envelope_generation() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let approval = approve_test_device(&mut store, &identity, "Laptop");
        let current = store
            .key_envelope_for_device(&approval.device.device_id)
            .expect("envelope reads")
            .expect("envelope exists");
        assert_eq!(current.rotation_generation, 0);

        let intent =
            devbox_auth::create_device_rotation_intent(devbox_auth::DeviceRotationIntentInput {
                account_id: &identity.account_id,
                device_id: &approval.device.device_id,
                requested_by_session_id: None,
                reason: "recovery rotation",
                created_at: "2026-06-18T10:02:00Z",
                now_unix: 102,
                ttl_seconds: 600,
                current_generation: current.rotation_generation,
            })
            .expect("intent creates");
        store
            .upsert_device_rotation_intent(&intent)
            .expect("intent persists");
        let (completed, rotated) = store
            .rotate_key_envelope_for_device(
                &intent,
                &identity.sync_key_hex,
                "2026-06-18T10:03:00Z",
                103,
            )
            .expect("envelope rotates");

        assert_eq!(completed.status, "completed");
        assert_eq!(completed.key_envelope_generation, 1);
        assert_eq!(rotated.rotation_generation, 1);
        let loaded = store
            .key_envelope_for_device(&approval.device.device_id)
            .expect("envelope reads")
            .expect("envelope exists");
        assert_eq!(loaded.rotation_generation, 1);
        let opened = devbox_auth::open_key_envelope(
            &loaded,
            &approval.device.device_key_hex,
            &approval.device.device_id,
        )
        .expect("rotated envelope opens");
        assert_eq!(opened, identity.sync_key_hex);

        let repeated = store.rotate_key_envelope_for_device(
            &completed,
            &identity.sync_key_hex,
            "2026-06-18T10:04:00Z",
            104,
        );
        assert!(matches!(repeated, Err(StoreError::InvalidStoredValue(_))));
        let stale_pending_reuse = store.rotate_key_envelope_for_device(
            &intent,
            &identity.sync_key_hex,
            "2026-06-18T10:05:00Z",
            105,
        );
        assert!(matches!(
            stale_pending_reuse,
            Err(StoreError::InvalidStoredValue(_))
        ));
        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "device_rotation_intents"), 1);
    }

    #[test]
    fn production_device_rotation_rejects_unpersisted_and_expired_intents() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let approval = approve_test_device(&mut store, &identity, "Laptop");
        let current = store
            .key_envelope_for_device(&approval.device.device_id)
            .expect("envelope reads")
            .expect("envelope exists");

        let never_persisted = rotation_intent_for(
            &identity.account_id,
            &approval.device.device_id,
            current.rotation_generation,
            100,
            600,
        );
        let missing = store.rotate_key_envelope_for_device(
            &never_persisted,
            &identity.sync_key_hex,
            "2026-06-18T10:02:00Z",
            102,
        );
        assert!(matches!(missing, Err(StoreError::InvalidStoredValue(_))));
        assert_eq!(
            store
                .key_envelope_for_device(&approval.device.device_id)
                .expect("envelope reads")
                .expect("envelope exists")
                .rotation_generation,
            0
        );

        let expired = rotation_intent_for(
            &identity.account_id,
            &approval.device.device_id,
            current.rotation_generation,
            100,
            1,
        );
        store
            .upsert_device_rotation_intent(&expired)
            .expect("expired intent persists");
        let expired_result = store.rotate_key_envelope_for_device(
            &expired,
            &identity.sync_key_hex,
            "2026-06-18T10:02:00Z",
            101,
        );
        assert!(matches!(
            expired_result,
            Err(StoreError::InvalidStoredValue(_))
        ));
        assert_eq!(
            store
                .key_envelope_for_device(&approval.device.device_id)
                .expect("envelope reads")
                .expect("envelope exists")
                .rotation_generation,
            0
        );
    }

    #[test]
    fn production_device_rotation_rejects_stale_generation_race() {
        let mut store = migrated_store();
        let identity = store
            .ensure_local_identity(&EnsureLocalIdentityOptions {
                device_name: Some("Current machine"),
            })
            .expect("identity initializes");
        let approval = approve_test_device(&mut store, &identity, "Laptop");
        let current = store
            .key_envelope_for_device(&approval.device.device_id)
            .expect("envelope reads")
            .expect("envelope exists");

        let first = rotation_intent_for(
            &identity.account_id,
            &approval.device.device_id,
            current.rotation_generation,
            100,
            600,
        );
        let second = rotation_intent_for(
            &identity.account_id,
            &approval.device.device_id,
            current.rotation_generation,
            100,
            600,
        );
        store
            .upsert_device_rotation_intent(&first)
            .expect("first intent persists");
        store
            .upsert_device_rotation_intent(&second)
            .expect("second intent persists");

        store
            .rotate_key_envelope_for_device(
                &first,
                &identity.sync_key_hex,
                "2026-06-18T10:02:00Z",
                102,
            )
            .expect("first intent rotates");
        let stale = store.rotate_key_envelope_for_device(
            &second,
            &identity.sync_key_hex,
            "2026-06-18T10:03:00Z",
            103,
        );
        assert!(matches!(stale, Err(StoreError::InvalidStoredValue(_))));
        assert_eq!(
            store
                .key_envelope_for_device(&approval.device.device_id)
                .expect("envelope reads")
                .expect("envelope exists")
                .rotation_generation,
            1
        );
        assert_eq!(
            store
                .device_rotation_intent(&second.id)
                .expect("intent reads")
                .expect("intent exists")
                .status,
            "pending"
        );
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
    fn conflict_records_persist_idempotently_with_deterministic_rows() {
        let mut store = migrated_store();
        let include = PolicyDecision::Include;
        let base_blob = blob_id_for(b"base");
        let local_blob = blob_id_for(b"local");
        let incoming_blob = blob_id_for(b"incoming");
        let base_path = Path::new("app.txt");
        store
            .persist_draft_snapshot(&draft(
                "snapshot-base",
                &[NewSnapshotManifestEntry {
                    relative_path: base_path,
                    kind: ManifestEntryKind::File,
                    size_bytes: 4,
                    blob_id: Some(&base_blob),
                    object_ref: Some("blobs/b3/base"),
                    policy_decision: &include,
                }],
            ))
            .expect("base persists");
        store
            .persist_draft_snapshot(&draft(
                "snapshot-local",
                &[NewSnapshotManifestEntry {
                    relative_path: base_path,
                    kind: ManifestEntryKind::File,
                    size_bytes: 5,
                    blob_id: Some(&local_blob),
                    object_ref: Some("blobs/b3/local"),
                    policy_decision: &include,
                }],
            ))
            .expect("local persists");
        store
            .persist_draft_snapshot(&draft(
                "snapshot-incoming",
                &[NewSnapshotManifestEntry {
                    relative_path: base_path,
                    kind: ManifestEntryKind::File,
                    size_bytes: 8,
                    blob_id: Some(&incoming_blob),
                    object_ref: Some("blobs/b3/incoming"),
                    policy_decision: &include,
                }],
            ))
            .expect("incoming persists");

        let summary = ConflictSummary::from_rows(&[]);
        let row = NewConflictRow {
            path: base_path,
            state: PathComparisonState::BothModifiedDifferent,
            entry_kind: &ManifestEntryKind::File,
            base_blob_id: Some(&base_blob),
            local_blob_id: Some(&local_blob),
            incoming_blob_id: Some(&incoming_blob),
            base_size_bytes: Some(4),
            local_size_bytes: Some(5),
            incoming_size_bytes: Some(8),
            local_policy_decision: Some(&include),
            incoming_policy_decision: Some(&include),
        };
        let conflict = NewConflict {
            id: "conflict-1",
            project_id: "project-1",
            base_snapshot_id: Some("snapshot-base"),
            local_snapshot_id: "snapshot-local",
            incoming_snapshot_id: "snapshot-incoming",
            summary: &summary,
            created_at: "2026-06-18T10:10:00Z",
        };

        let first = store
            .persist_conflict(&conflict, std::slice::from_ref(&row))
            .expect("conflict persists");
        let second = store
            .persist_conflict(&conflict, std::slice::from_ref(&row))
            .expect("duplicate create returns existing conflict");

        assert_eq!(first.conflict.id, "conflict-1");
        assert_eq!(second.conflict.id, "conflict-1");
        assert_eq!(first.rows.len(), 1);
        assert_eq!(second.rows.len(), 1);
        assert_eq!(
            second.rows[0].state,
            PathComparisonState::BothModifiedDifferent
        );
        let listed = store
            .list_conflicts(Some("project-1"))
            .expect("conflicts list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, ConflictStatus::Open);

        let summary = store.schema_summary().expect("summary reads");
        assert_eq!(count(&summary, "conflicts"), 1);
        assert_eq!(count(&summary, "conflict_rows"), 1);
    }

    #[test]
    fn conflict_status_transitions_are_small_and_explicit() {
        let mut store = migrated_store();
        let include = PolicyDecision::Include;
        let blob = blob_id_for(b"same");
        let path = Path::new("README.md");
        let entries = [NewSnapshotManifestEntry {
            relative_path: path,
            kind: ManifestEntryKind::File,
            size_bytes: 4,
            blob_id: Some(&blob),
            object_ref: Some("blobs/b3/same"),
            policy_decision: &include,
        }];
        store
            .persist_draft_snapshot(&draft("snapshot-local", &entries))
            .expect("local persists");
        store
            .persist_draft_snapshot(&draft("snapshot-incoming", &entries))
            .expect("incoming persists");
        let summary = ConflictSummary::from_rows(&[]);
        store
            .persist_conflict(
                &NewConflict {
                    id: "conflict-status",
                    project_id: "project-1",
                    base_snapshot_id: None,
                    local_snapshot_id: "snapshot-local",
                    incoming_snapshot_id: "snapshot-incoming",
                    summary: &summary,
                    created_at: "2026-06-18T10:10:00Z",
                },
                &[],
            )
            .expect("conflict persists");

        let resolved = store
            .update_conflict_status(
                "conflict-status",
                ConflictStatus::Resolved,
                "2026-06-18T10:11:00Z",
            )
            .expect("status updates")
            .expect("conflict exists");

        assert_eq!(resolved.status, ConflictStatus::Resolved);
        assert_eq!(resolved.updated_at, "2026-06-18T10:11:00Z");
        assert!(store
            .update_conflict_status(
                "missing-conflict",
                ConflictStatus::Dismissed,
                "2026-06-18T10:12:00Z",
            )
            .expect("missing status update is handled")
            .is_none());
    }

    #[test]
    fn conflict_foreign_keys_reject_missing_snapshots() {
        let mut store = migrated_store();
        store
            .insert_project(&NewProject {
                id: "project-1",
                root_path: "/workspace/devbox",
                kind: "Rust",
                display_name: "devbox",
                discovered_at: "2026-06-18T10:00:00Z",
            })
            .expect("project inserts");
        let summary = ConflictSummary::from_rows(&[]);
        let result = store.persist_conflict(
            &NewConflict {
                id: "conflict-missing-snapshots",
                project_id: "project-1",
                base_snapshot_id: None,
                local_snapshot_id: "missing-local",
                incoming_snapshot_id: "missing-incoming",
                summary: &summary,
                created_at: "2026-06-18T10:10:00Z",
            },
            &[],
        );

        assert!(matches!(
            result,
            Err(StoreError::Sqlite(rusqlite::Error::SqliteFailure(
                error,
                _
            ))) if error.code == rusqlite::ErrorCode::ConstraintViolation
        ));
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

    fn rotation_intent_for(
        account_id: &str,
        device_id: &str,
        current_generation: u64,
        now_unix: i64,
        ttl_seconds: i64,
    ) -> DeviceRotationIntent {
        devbox_auth::create_device_rotation_intent(devbox_auth::DeviceRotationIntentInput {
            account_id,
            device_id,
            requested_by_session_id: None,
            reason: "recovery rotation",
            created_at: "2026-06-18T10:02:00Z",
            now_unix,
            ttl_seconds,
            current_generation,
        })
        .expect("rotation intent creates")
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

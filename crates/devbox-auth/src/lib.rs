//! Local/mock auth and device-pairing domain primitives for Phase 1 foundations.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const ENVELOPE_MAGIC: &[u8] = b"devbox-key-envelope-v1\n";
const TOKEN_PREFIX: &str = "devbox-pair-v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    InvalidOwnershipProof {
        field: &'static str,
        reason: &'static str,
    },
    InvalidHex {
        field: &'static str,
    },
    InvalidSessionToken,
    AccountSessionExpired {
        session_id: String,
    },
    AccountSessionRevoked {
        session_id: String,
    },
    AccountSessionTokenMismatch {
        session_id: String,
    },
    MalformedInvitation,
    InvitationSecretMismatch,
    InvitationExpired {
        invitation_id: String,
    },
    InvitationNotPending {
        invitation_id: String,
        status: String,
    },
    AccountMismatch {
        expected: String,
        actual: String,
    },
    DeviceAlreadyRevoked {
        device_id: String,
    },
    InvalidRecoveryGrant {
        field: &'static str,
        reason: &'static str,
    },
    RecoveryGrantExpired {
        grant_id: String,
    },
    RecoveryGrantRevoked {
        grant_id: String,
    },
    RotationIntentNotPending {
        intent_id: String,
        status: String,
    },
    Crypto,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOwnershipProof { field, reason } => {
                write!(
                    f,
                    "{field} is not a valid account ownership proof field: {reason}"
                )
            }
            Self::InvalidHex { field } => write!(f, "{field} must be 64 hex characters"),
            Self::InvalidSessionToken => f.write_str("session token must not be empty"),
            Self::AccountSessionExpired { session_id } => {
                write!(f, "account session expired: {session_id}")
            }
            Self::AccountSessionRevoked { session_id } => {
                write!(f, "account session revoked: {session_id}")
            }
            Self::AccountSessionTokenMismatch { session_id } => {
                write!(f, "account session token hash mismatch: {session_id}")
            }
            Self::MalformedInvitation => f.write_str("pairing invitation is malformed"),
            Self::InvitationSecretMismatch => {
                f.write_str("pairing invitation secret does not match stored invitation")
            }
            Self::InvitationExpired { invitation_id } => {
                write!(f, "pairing invitation expired: {invitation_id}")
            }
            Self::InvitationNotPending {
                invitation_id,
                status,
            } => write!(
                f,
                "pairing invitation {invitation_id} is not pending; current status is {status}"
            ),
            Self::AccountMismatch { expected, actual } => {
                write!(f, "account mismatch: expected {expected}, got {actual}")
            }
            Self::DeviceAlreadyRevoked { device_id } => {
                write!(f, "device is already revoked: {device_id}")
            }
            Self::InvalidRecoveryGrant { field, reason } => {
                write!(
                    f,
                    "{field} is not a valid recovery/rotation field: {reason}"
                )
            }
            Self::RecoveryGrantExpired { grant_id } => {
                write!(f, "recovery grant expired: {grant_id}")
            }
            Self::RecoveryGrantRevoked { grant_id } => {
                write!(f, "recovery grant revoked: {grant_id}")
            }
            Self::RotationIntentNotPending { intent_id, status } => write!(
                f,
                "device rotation intent {intent_id} is not pending; current status is {status}"
            ),
            Self::Crypto => f.write_str("auth crypto operation failed"),
        }
    }
}

impl std::error::Error for AuthError {}

pub type AuthResult<T> = Result<T, AuthError>;

#[derive(Clone, PartialEq, Eq)]
pub struct LocalIdentityView {
    pub account_id: String,
    pub device_id: String,
    pub device_name: String,
    pub sync_key_hex: String,
}

impl fmt::Debug for LocalIdentityView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalIdentityView")
            .field("account_id", &self.account_id)
            .field("device_id", &self.device_id)
            .field("device_name", &self.device_name)
            .field("sync_key_hex", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSession {
    pub account_id: String,
    pub provider_kind: String,
    pub subject: String,
    pub session_state: String,
    pub proof_issued_at: String,
    pub last_refreshed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountOwnershipProof {
    pub account_id: String,
    pub provider_kind: String,
    pub provider_issuer: String,
    pub provider_subject: String,
    pub verified_email: Option<String>,
    pub verified_domain: Option<String>,
    pub proof_state: String,
    pub proof_issued_at: String,
    pub proof_expires_at_unix: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountOwnershipProofInput<'a> {
    pub account_id: &'a str,
    pub provider_kind: &'a str,
    pub provider_issuer: &'a str,
    pub provider_subject: &'a str,
    pub verified_email: Option<&'a str>,
    pub verified_domain: Option<&'a str>,
    pub proof_issued_at: &'a str,
    pub proof_expires_at_unix: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AccountSession {
    pub session_id: String,
    pub account_id: String,
    pub provider_kind: String,
    pub provider_issuer: String,
    pub provider_subject: String,
    pub session_token_hash_hex: String,
    pub session_state: String,
    pub created_at: String,
    pub expires_at_unix: i64,
    pub revoked_at: Option<String>,
    pub last_seen_at: String,
}

impl fmt::Debug for AccountSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountSession")
            .field("session_id", &self.session_id)
            .field("account_id", &self.account_id)
            .field("provider_kind", &self.provider_kind)
            .field("provider_issuer", &self.provider_issuer)
            .field("provider_subject", &self.provider_subject)
            .field("session_token_hash_hex", &"<redacted>")
            .field("session_state", &self.session_state)
            .field("created_at", &self.created_at)
            .field("expires_at_unix", &self.expires_at_unix)
            .field("revoked_at", &self.revoked_at)
            .field("last_seen_at", &self.last_seen_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedAccountSession {
    pub account_id: String,
    pub session_id: String,
    pub provider_kind: String,
    pub provider_issuer: String,
    pub provider_subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingInvitation {
    pub id: String,
    pub account_id: String,
    pub inviter_device_id: String,
    pub secret_hash_hex: String,
    pub status: String,
    pub created_at: String,
    pub expires_at_unix: i64,
    pub approved_device_id: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PairingInvitationToken {
    pub id: String,
    pub account_id: String,
    pub inviter_device_id: String,
    pub expires_at_unix: i64,
    secret_hex: String,
}

impl fmt::Debug for PairingInvitationToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingInvitationToken")
            .field("id", &self.id)
            .field("account_id", &self.account_id)
            .field("inviter_device_id", &self.inviter_device_id)
            .field("expires_at_unix", &self.expires_at_unix)
            .field("secret_hex", &"<redacted>")
            .finish()
    }
}

impl PairingInvitationToken {
    pub fn parse(value: &str) -> AuthResult<Self> {
        let fields = value.split(':').collect::<Vec<_>>();
        if fields.len() != 6 || fields[0] != TOKEN_PREFIX {
            return Err(AuthError::MalformedInvitation);
        }
        let expires_at_unix = fields[4]
            .parse::<i64>()
            .map_err(|_| AuthError::MalformedInvitation)?;
        if fields[1].is_empty()
            || fields[2].is_empty()
            || fields[3].is_empty()
            || decode_key_hex(fields[5], "pairing invitation secret").is_err()
        {
            return Err(AuthError::MalformedInvitation);
        }

        Ok(Self {
            id: fields[1].to_string(),
            account_id: fields[2].to_string(),
            inviter_device_id: fields[3].to_string(),
            expires_at_unix,
            secret_hex: fields[5].to_string(),
        })
    }

    pub fn expose_for_cli(&self) -> String {
        format!(
            "{TOKEN_PREFIX}:{}:{}:{}:{}:{}",
            self.id, self.account_id, self.inviter_device_id, self.expires_at_unix, self.secret_hex
        )
    }

    pub fn secret_hash_hex(&self) -> String {
        hash_secret_hex(&self.secret_hex)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingInvitationDraft {
    pub invitation: PairingInvitation,
    pub token: PairingInvitationToken,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ApprovedDevice {
    pub device_id: String,
    pub account_id: String,
    pub display_name: String,
    pub device_key_hex: String,
    pub invitation_id: String,
    pub approved_at: String,
}

impl fmt::Debug for ApprovedDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApprovedDevice")
            .field("device_id", &self.device_id)
            .field("account_id", &self.account_id)
            .field("display_name", &self.display_name)
            .field("device_key_hex", &"<redacted>")
            .field("invitation_id", &self.invitation_id)
            .field("approved_at", &self.approved_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEnvelope {
    pub id: String,
    pub account_id: String,
    pub device_id: String,
    pub key_ref: String,
    pub ciphertext_hex: String,
    pub created_at: String,
    pub rotation_generation: u64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PairingApproval {
    pub device: ApprovedDevice,
    pub envelope: KeyEnvelope,
}

impl fmt::Debug for PairingApproval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingApproval")
            .field("device", &self.device)
            .field("envelope", &self.envelope)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustRecord {
    pub device_id: String,
    pub account_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub trust_state: String,
    pub approved_at: Option<String>,
    pub revoked_at: Option<String>,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceProjectCursor {
    pub account_id: String,
    pub device_id: String,
    pub project_id: String,
    pub cursor_value: String,
    pub updated_at: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RecoveryGrant {
    pub id: String,
    pub account_id: String,
    pub device_id: String,
    pub grant_ref: String,
    pub status: String,
    pub created_at: String,
    pub expires_at_unix: i64,
    pub consumed_at: Option<String>,
    pub revoked_at: Option<String>,
    pub audit_label: String,
}

impl fmt::Debug for RecoveryGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryGrant")
            .field("id", &self.id)
            .field("account_id", &self.account_id)
            .field("device_id", &self.device_id)
            .field("grant_ref", &self.grant_ref)
            .field("status", &self.status)
            .field("created_at", &self.created_at)
            .field("expires_at_unix", &self.expires_at_unix)
            .field("consumed_at", &self.consumed_at)
            .field("revoked_at", &self.revoked_at)
            .field("audit_label", &self.audit_label)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRotationIntent {
    pub id: String,
    pub account_id: String,
    pub device_id: String,
    pub requested_by_session_id: Option<String>,
    pub status: String,
    pub reason: String,
    pub created_at: String,
    pub expires_at_unix: i64,
    pub completed_at: Option<String>,
    pub revoked_at: Option<String>,
    pub key_envelope_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceRotationIntentInput<'a> {
    pub account_id: &'a str,
    pub device_id: &'a str,
    pub requested_by_session_id: Option<&'a str>,
    pub reason: &'a str,
    pub created_at: &'a str,
    pub now_unix: i64,
    pub ttl_seconds: i64,
    pub current_generation: u64,
}

pub fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub fn mock_login(identity: &LocalIdentityView, now: &str) -> AuthSession {
    AuthSession {
        account_id: identity.account_id.clone(),
        provider_kind: "local-mock".to_string(),
        subject: format!("local-dev:{}", identity.account_id),
        session_state: "active".to_string(),
        proof_issued_at: now.to_string(),
        last_refreshed_at: now.to_string(),
    }
}

pub fn create_account_ownership_proof(
    input: AccountOwnershipProofInput<'_>,
) -> AuthResult<AccountOwnershipProof> {
    let account_id = public_identifier(input.account_id, "account id")?;
    let provider_kind = public_identifier(input.provider_kind, "provider kind")?;
    let provider_issuer = public_identifier(input.provider_issuer, "provider issuer")?;
    let provider_subject = public_identifier(input.provider_subject, "provider subject")?;
    let verified_email = optional_public_identifier(input.verified_email, "verified email")?;
    let verified_domain = optional_public_identifier(input.verified_domain, "verified domain")?;
    if verified_email.is_none() && verified_domain.is_none() {
        return Err(AuthError::InvalidOwnershipProof {
            field: "verified email/domain",
            reason: "at least one verified email or domain is required",
        });
    }
    if input.proof_issued_at.trim().is_empty() {
        return Err(AuthError::InvalidOwnershipProof {
            field: "proof issued at",
            reason: "value must not be empty",
        });
    }
    if input.proof_expires_at_unix <= 0 {
        return Err(AuthError::InvalidOwnershipProof {
            field: "proof expires at",
            reason: "expiration must be a positive unix timestamp",
        });
    }

    Ok(AccountOwnershipProof {
        account_id,
        provider_kind,
        provider_issuer,
        provider_subject,
        verified_email,
        verified_domain,
        proof_state: "verified".to_string(),
        proof_issued_at: input.proof_issued_at.to_string(),
        proof_expires_at_unix: input.proof_expires_at_unix,
    })
}

pub fn validate_account_ownership_proof(
    proof: &AccountOwnershipProof,
    now_unix: i64,
) -> AuthResult<()> {
    public_identifier(&proof.account_id, "account id")?;
    public_identifier(&proof.provider_kind, "provider kind")?;
    public_identifier(&proof.provider_issuer, "provider issuer")?;
    public_identifier(&proof.provider_subject, "provider subject")?;
    optional_public_identifier(proof.verified_email.as_deref(), "verified email")?;
    optional_public_identifier(proof.verified_domain.as_deref(), "verified domain")?;
    if proof.proof_state != "verified" {
        return Err(AuthError::InvalidOwnershipProof {
            field: "proof state",
            reason: "only verified ownership proofs are accepted",
        });
    }
    if proof.verified_email.is_none() && proof.verified_domain.is_none() {
        return Err(AuthError::InvalidOwnershipProof {
            field: "verified email/domain",
            reason: "at least one verified email or domain is required",
        });
    }
    if proof.proof_expires_at_unix <= now_unix {
        return Err(AuthError::InvalidOwnershipProof {
            field: "proof expires at",
            reason: "ownership proof is expired",
        });
    }
    Ok(())
}

pub fn create_account_session(
    proof: &AccountOwnershipProof,
    raw_session_token: &str,
    created_at: &str,
    now_unix: i64,
    ttl_seconds: i64,
) -> AuthResult<AccountSession> {
    validate_account_ownership_proof(proof, now_unix)?;
    if raw_session_token.trim().is_empty() {
        return Err(AuthError::InvalidSessionToken);
    }
    if created_at.trim().is_empty() {
        return Err(AuthError::InvalidOwnershipProof {
            field: "session created at",
            reason: "value must not be empty",
        });
    }
    if ttl_seconds <= 0 {
        return Err(AuthError::InvalidOwnershipProof {
            field: "session ttl",
            reason: "ttl must be positive",
        });
    }

    Ok(AccountSession {
        session_id: random_prefixed_id("session")?,
        account_id: proof.account_id.clone(),
        provider_kind: proof.provider_kind.clone(),
        provider_issuer: proof.provider_issuer.clone(),
        provider_subject: proof.provider_subject.clone(),
        session_token_hash_hex: hash_session_token_hex(raw_session_token),
        session_state: "active".to_string(),
        created_at: created_at.to_string(),
        expires_at_unix: now_unix + ttl_seconds,
        revoked_at: None,
        last_seen_at: created_at.to_string(),
    })
}

pub fn validate_account_session(
    session: &AccountSession,
    raw_session_token: &str,
    now_unix: i64,
) -> AuthResult<AuthenticatedAccountSession> {
    if raw_session_token.trim().is_empty() {
        return Err(AuthError::InvalidSessionToken);
    }
    validate_account_session_hash(
        session,
        &hash_session_token_hex(raw_session_token),
        now_unix,
    )
}

pub fn validate_account_session_hash(
    session: &AccountSession,
    session_token_hash_hex: &str,
    now_unix: i64,
) -> AuthResult<AuthenticatedAccountSession> {
    if session.session_state == "revoked" || session.revoked_at.is_some() {
        return Err(AuthError::AccountSessionRevoked {
            session_id: session.session_id.clone(),
        });
    }
    if session.expires_at_unix <= now_unix {
        return Err(AuthError::AccountSessionExpired {
            session_id: session.session_id.clone(),
        });
    }
    if session.session_token_hash_hex != session_token_hash_hex {
        return Err(AuthError::AccountSessionTokenMismatch {
            session_id: session.session_id.clone(),
        });
    }

    Ok(AuthenticatedAccountSession {
        account_id: session.account_id.clone(),
        session_id: session.session_id.clone(),
        provider_kind: session.provider_kind.clone(),
        provider_issuer: session.provider_issuer.clone(),
        provider_subject: session.provider_subject.clone(),
    })
}

pub fn revoke_account_session(
    session: &AccountSession,
    revoked_at: &str,
) -> AuthResult<AccountSession> {
    if session.session_state == "revoked" || session.revoked_at.is_some() {
        return Err(AuthError::AccountSessionRevoked {
            session_id: session.session_id.clone(),
        });
    }
    let mut revoked = session.clone();
    revoked.session_state = "revoked".to_string();
    revoked.revoked_at = Some(revoked_at.to_string());
    revoked.last_seen_at = revoked_at.to_string();
    Ok(revoked)
}

pub fn hash_session_token_hex(raw_session_token: &str) -> String {
    blake3::hash(raw_session_token.as_bytes())
        .to_hex()
        .to_string()
}

pub fn create_pairing_invitation(
    identity: &LocalIdentityView,
    now: &str,
    now_unix: i64,
    ttl_seconds: i64,
) -> AuthResult<PairingInvitationDraft> {
    let secret_hex = random_key_hex("pairing invitation secret")?;
    let token = PairingInvitationToken {
        id: random_prefixed_id("pairing")?,
        account_id: identity.account_id.clone(),
        inviter_device_id: identity.device_id.clone(),
        expires_at_unix: now_unix + ttl_seconds,
        secret_hex,
    };
    let invitation = PairingInvitation {
        id: token.id.clone(),
        account_id: token.account_id.clone(),
        inviter_device_id: token.inviter_device_id.clone(),
        secret_hash_hex: token.secret_hash_hex(),
        status: "pending".to_string(),
        created_at: now.to_string(),
        expires_at_unix: token.expires_at_unix,
        approved_device_id: None,
    };

    Ok(PairingInvitationDraft { invitation, token })
}

pub fn approve_pairing_invitation(
    identity: &LocalIdentityView,
    stored: &PairingInvitation,
    token: &PairingInvitationToken,
    device_name: &str,
    now: &str,
    now_unix: i64,
) -> AuthResult<PairingApproval> {
    if stored.status != "pending" {
        return Err(AuthError::InvitationNotPending {
            invitation_id: stored.id.clone(),
            status: stored.status.clone(),
        });
    }
    if stored.account_id != token.account_id || stored.account_id != identity.account_id {
        return Err(AuthError::AccountMismatch {
            expected: stored.account_id.clone(),
            actual: token.account_id.clone(),
        });
    }
    if stored.id != token.id || stored.inviter_device_id != token.inviter_device_id {
        return Err(AuthError::MalformedInvitation);
    }
    if stored.expires_at_unix <= now_unix {
        return Err(AuthError::InvitationExpired {
            invitation_id: stored.id.clone(),
        });
    }
    if stored.secret_hash_hex != token.secret_hash_hex() {
        return Err(AuthError::InvitationSecretMismatch);
    }

    let display_name = if device_name.trim().is_empty() {
        "approved device"
    } else {
        device_name.trim()
    };
    let device_key_hex = random_key_hex("approved device key")?;
    let device_id = random_prefixed_id("device")?;
    let envelope = create_key_envelope(
        &identity.account_id,
        &device_id,
        &device_key_hex,
        &identity.sync_key_hex,
        now,
    )?;

    Ok(PairingApproval {
        device: ApprovedDevice {
            device_id,
            account_id: identity.account_id.clone(),
            display_name: display_name.to_string(),
            device_key_hex,
            invitation_id: stored.id.clone(),
            approved_at: now.to_string(),
        },
        envelope,
    })
}

pub fn create_key_envelope(
    account_id: &str,
    device_id: &str,
    device_key_hex: &str,
    sync_key_hex: &str,
    now: &str,
) -> AuthResult<KeyEnvelope> {
    let device_key = decode_key_hex(device_key_hex, "device key")?;
    let sync_key = decode_key_hex(sync_key_hex, "sync key")?;
    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| AuthError::Crypto)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&device_key));
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &sync_key,
                aad: device_id.as_bytes(),
            },
        )
        .map_err(|_| AuthError::Crypto)?;

    let mut envelope = Vec::with_capacity(ENVELOPE_MAGIC.len() + NONCE_LEN + ciphertext.len());
    envelope.extend_from_slice(ENVELOPE_MAGIC);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&ciphertext);

    Ok(KeyEnvelope {
        id: random_prefixed_id("envelope")?,
        account_id: account_id.to_string(),
        device_id: device_id.to_string(),
        key_ref: "account-sync-key-v1".to_string(),
        ciphertext_hex: hex_encode(&envelope),
        created_at: now.to_string(),
        rotation_generation: 0,
    })
}

pub fn open_key_envelope(
    envelope: &KeyEnvelope,
    device_key_hex: &str,
    device_id: &str,
) -> AuthResult<String> {
    let device_key = decode_key_hex(device_key_hex, "device key")?;
    let envelope_bytes =
        hex_decode(&envelope.ciphertext_hex).ok_or(AuthError::InvalidHex { field: "envelope" })?;
    if envelope_bytes.len() < ENVELOPE_MAGIC.len() + NONCE_LEN
        || &envelope_bytes[..ENVELOPE_MAGIC.len()] != ENVELOPE_MAGIC
    {
        return Err(AuthError::Crypto);
    }
    let nonce_start = ENVELOPE_MAGIC.len();
    let ciphertext_start = nonce_start + NONCE_LEN;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(&device_key));
    let plaintext = cipher
        .decrypt(
            XNonce::from_slice(&envelope_bytes[nonce_start..ciphertext_start]),
            Payload {
                msg: &envelope_bytes[ciphertext_start..],
                aad: device_id.as_bytes(),
            },
        )
        .map_err(|_| AuthError::Crypto)?;
    if plaintext.len() != KEY_LEN {
        return Err(AuthError::Crypto);
    }
    Ok(hex_encode(&plaintext))
}

pub fn revoke_trusted_device(device: &DeviceTrustRecord) -> AuthResult<()> {
    if device.trust_state == "revoked" {
        return Err(AuthError::DeviceAlreadyRevoked {
            device_id: device.device_id.clone(),
        });
    }
    Ok(())
}

pub fn create_recovery_grant(
    account_id: &str,
    device_id: &str,
    grant_ref: &str,
    audit_label: &str,
    now: &str,
    now_unix: i64,
    ttl_seconds: i64,
) -> AuthResult<RecoveryGrant> {
    let account_id = recovery_public_identifier(account_id, "account id")?;
    let device_id = recovery_public_identifier(device_id, "device id")?;
    let grant_ref = recovery_reference(grant_ref, "recovery grant reference")?;
    let audit_label = recovery_public_identifier(audit_label, "audit label")?;
    if now.trim().is_empty() {
        return Err(AuthError::InvalidRecoveryGrant {
            field: "created at",
            reason: "value must not be empty",
        });
    }
    if ttl_seconds <= 0 {
        return Err(AuthError::InvalidRecoveryGrant {
            field: "ttl",
            reason: "ttl must be positive",
        });
    }

    Ok(RecoveryGrant {
        id: random_prefixed_id("recovery")?,
        account_id,
        device_id,
        grant_ref,
        status: "pending".to_string(),
        created_at: now.to_string(),
        expires_at_unix: now_unix + ttl_seconds,
        consumed_at: None,
        revoked_at: None,
        audit_label,
    })
}

pub fn consume_recovery_grant(
    grant: &RecoveryGrant,
    now: &str,
    now_unix: i64,
) -> AuthResult<RecoveryGrant> {
    ensure_recovery_grant_active(grant, now_unix)?;
    let mut consumed = grant.clone();
    consumed.status = "consumed".to_string();
    consumed.consumed_at = Some(now.to_string());
    Ok(consumed)
}

pub fn revoke_recovery_grant(grant: &RecoveryGrant, now: &str) -> AuthResult<RecoveryGrant> {
    let mut revoked = grant.clone();
    if revoked.revoked_at.is_none() {
        revoked.status = "revoked".to_string();
        revoked.revoked_at = Some(now.to_string());
    }
    Ok(revoked)
}

pub fn create_device_rotation_intent(
    input: DeviceRotationIntentInput<'_>,
) -> AuthResult<DeviceRotationIntent> {
    let account_id = recovery_public_identifier(input.account_id, "account id")?;
    let device_id = recovery_public_identifier(input.device_id, "device id")?;
    let requested_by_session_id = input
        .requested_by_session_id
        .map(|value| recovery_public_identifier(value, "requested by session id"))
        .transpose()?;
    let reason = recovery_public_identifier(input.reason, "reason")?;
    if input.created_at.trim().is_empty() {
        return Err(AuthError::InvalidRecoveryGrant {
            field: "created at",
            reason: "value must not be empty",
        });
    }
    if input.ttl_seconds <= 0 {
        return Err(AuthError::InvalidRecoveryGrant {
            field: "ttl",
            reason: "ttl must be positive",
        });
    }

    Ok(DeviceRotationIntent {
        id: random_prefixed_id("rotation")?,
        account_id,
        device_id,
        requested_by_session_id,
        status: "pending".to_string(),
        reason,
        created_at: input.created_at.to_string(),
        expires_at_unix: input.now_unix + input.ttl_seconds,
        completed_at: None,
        revoked_at: None,
        key_envelope_generation: input.current_generation,
    })
}

pub fn complete_device_rotation_intent(
    intent: &DeviceRotationIntent,
    completed_at: &str,
    next_generation: u64,
) -> AuthResult<DeviceRotationIntent> {
    if intent.status != "pending" {
        return Err(AuthError::RotationIntentNotPending {
            intent_id: intent.id.clone(),
            status: intent.status.clone(),
        });
    }
    let mut completed = intent.clone();
    completed.status = "completed".to_string();
    completed.completed_at = Some(completed_at.to_string());
    completed.key_envelope_generation = next_generation;
    Ok(completed)
}

pub fn revoke_device_rotation_intent(
    intent: &DeviceRotationIntent,
    revoked_at: &str,
) -> AuthResult<DeviceRotationIntent> {
    let mut revoked = intent.clone();
    if revoked.revoked_at.is_none() && revoked.status == "pending" {
        revoked.status = "revoked".to_string();
        revoked.revoked_at = Some(revoked_at.to_string());
    }
    Ok(revoked)
}

fn ensure_recovery_grant_active(grant: &RecoveryGrant, now_unix: i64) -> AuthResult<()> {
    if grant.status == "revoked" || grant.revoked_at.is_some() {
        return Err(AuthError::RecoveryGrantRevoked {
            grant_id: grant.id.clone(),
        });
    }
    if grant.expires_at_unix <= now_unix {
        return Err(AuthError::RecoveryGrantExpired {
            grant_id: grant.id.clone(),
        });
    }
    Ok(())
}

fn hash_secret_hex(secret_hex: &str) -> String {
    blake3::hash(secret_hex.as_bytes()).to_hex().to_string()
}

fn public_identifier(value: &str, field: &'static str) -> AuthResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AuthError::InvalidOwnershipProof {
            field,
            reason: "value must not be empty",
        });
    }
    if contains_secret_marker(trimmed) {
        return Err(AuthError::InvalidOwnershipProof {
            field,
            reason: "value must not contain secret-looking material",
        });
    }
    Ok(trimmed.to_string())
}

fn recovery_public_identifier(value: &str, field: &'static str) -> AuthResult<String> {
    public_identifier(value, field).map_err(|_| AuthError::InvalidRecoveryGrant {
        field,
        reason: "value must be public metadata and must not contain secret-looking material",
    })
}

fn recovery_reference(value: &str, field: &'static str) -> AuthResult<String> {
    let trimmed = recovery_public_identifier(value, field)?;
    if !(trimmed.starts_with("recovery-ref:")
        || trimmed.starts_with("grant-ref:")
        || trimmed.starts_with("mock-recovery-ref:"))
    {
        return Err(AuthError::InvalidRecoveryGrant {
            field,
            reason: "reference must be a redacted recovery/grant reference",
        });
    }
    Ok(trimmed)
}

fn optional_public_identifier(
    value: Option<&str>,
    field: &'static str,
) -> AuthResult<Option<String>> {
    value
        .map(|value| public_identifier(value, field))
        .transpose()
}

fn contains_secret_marker(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    [
        "client_secret",
        "refresh_token",
        "access_token",
        "bearer ",
        "private_key",
        "credential",
        "secret",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
}

fn random_prefixed_id(prefix: &str) -> AuthResult<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).map_err(|_| AuthError::Crypto)?;
    Ok(format!("{prefix}-{}", hex_encode(&bytes)))
}

fn random_key_hex(field: &'static str) -> AuthResult<String> {
    let mut bytes = [0_u8; KEY_LEN];
    getrandom::getrandom(&mut bytes).map_err(|_| AuthError::InvalidHex { field })?;
    Ok(hex_encode(&bytes))
}

fn decode_key_hex(value: &str, field: &'static str) -> AuthResult<[u8; KEY_LEN]> {
    let bytes = hex_decode(value).ok_or(AuthError::InvalidHex { field })?;
    bytes
        .try_into()
        .map_err(|_| AuthError::InvalidHex { field })
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks_exact(2) {
        let high = hex_value(chunk[0])?;
        let low = hex_value(chunk[1])?;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
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

    fn identity() -> LocalIdentityView {
        LocalIdentityView {
            account_id: "account-test".to_string(),
            device_id: "device-local".to_string(),
            device_name: "Current".to_string(),
            sync_key_hex: "1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
        }
    }

    fn ownership_proof() -> AccountOwnershipProof {
        create_account_ownership_proof(AccountOwnershipProofInput {
            account_id: "account-test",
            provider_kind: "oidc-dev",
            provider_issuer: "https://issuer.devbox.local",
            provider_subject: "provider-subject-123",
            verified_email: Some("user@example.com"),
            verified_domain: Some("example.com"),
            proof_issued_at: "2026-06-18T10:00:00Z",
            proof_expires_at_unix: 1_000,
        })
        .expect("ownership proof validates")
    }

    #[test]
    fn ownership_proof_requires_verified_account_material_without_secret_markers() {
        let missing_verified_material =
            create_account_ownership_proof(AccountOwnershipProofInput {
                account_id: "account-test",
                provider_kind: "oidc-dev",
                provider_issuer: "https://issuer.devbox.local",
                provider_subject: "provider-subject-123",
                verified_email: None,
                verified_domain: None,
                proof_issued_at: "2026-06-18T10:00:00Z",
                proof_expires_at_unix: 1_000,
            })
            .expect_err("verified email or domain is required");
        assert!(matches!(
            missing_verified_material,
            AuthError::InvalidOwnershipProof {
                field: "verified email/domain",
                ..
            }
        ));

        let secret_like_subject = "provider-secret-should-not-persist";
        let secret = create_account_ownership_proof(AccountOwnershipProofInput {
            account_id: "account-test",
            provider_kind: "oidc-dev",
            provider_issuer: "https://issuer.devbox.local",
            provider_subject: secret_like_subject,
            verified_email: Some("user@example.com"),
            verified_domain: None,
            proof_issued_at: "2026-06-18T10:00:00Z",
            proof_expires_at_unix: 1_000,
        })
        .expect_err("secret-looking provider subject is rejected");

        assert!(!secret.to_string().contains(secret_like_subject));
        assert!(secret
            .to_string()
            .contains("value must not contain secret-looking material"));
    }

    #[test]
    fn account_session_hashes_token_and_validates_expiry_and_revocation() {
        let raw_token = "raw-dev-session-token";
        let proof = ownership_proof();
        let session = create_account_session(&proof, raw_token, "2026-06-18T10:01:00Z", 101, 600)
            .expect("session creates");

        assert!(session.session_id.starts_with("session-"));
        assert_eq!(session.session_token_hash_hex.len(), 64);
        assert_ne!(session.session_token_hash_hex, raw_token);
        assert_eq!(
            validate_account_session(&session, raw_token, 102)
                .expect("session validates")
                .account_id,
            proof.account_id
        );

        let wrong_token =
            validate_account_session(&session, "wrong-token", 102).expect_err("wrong token fails");
        assert!(matches!(
            wrong_token,
            AuthError::AccountSessionTokenMismatch { .. }
        ));

        let expired = validate_account_session(&session, raw_token, session.expires_at_unix)
            .expect_err("expired session fails");
        assert!(matches!(expired, AuthError::AccountSessionExpired { .. }));

        let revoked =
            revoke_account_session(&session, "2026-06-18T10:02:00Z").expect("session revokes");
        let revoked_check =
            validate_account_session(&revoked, raw_token, 102).expect_err("revoked session fails");
        assert!(matches!(
            revoked_check,
            AuthError::AccountSessionRevoked { .. }
        ));
    }

    #[test]
    fn account_session_debug_redacts_token_hash() {
        let raw_token = "raw-dev-session-token";
        let session = create_account_session(
            &ownership_proof(),
            raw_token,
            "2026-06-18T10:01:00Z",
            101,
            600,
        )
        .expect("session creates");

        let formatted = format!("{session:?}");

        assert!(!formatted.contains(raw_token));
        assert!(!formatted.contains(&session.session_token_hash_hex));
        assert!(formatted.contains("<redacted>"));
    }

    #[test]
    fn pairing_token_round_trips_without_leaking_secret_hash() {
        let draft = create_pairing_invitation(&identity(), "2026-06-18T10:00:00Z", 100, 600)
            .expect("invitation creates");

        let encoded = draft.token.expose_for_cli();
        let parsed = PairingInvitationToken::parse(&encoded).expect("token parses");

        assert_eq!(parsed.id, draft.invitation.id);
        assert_eq!(parsed.account_id, draft.invitation.account_id);
        assert_eq!(parsed.secret_hash_hex(), draft.invitation.secret_hash_hex);
        assert!(!encoded.contains(&draft.invitation.secret_hash_hex));
    }

    #[test]
    fn approval_rejects_malformed_expired_and_reused_invitations() {
        assert!(matches!(
            PairingInvitationToken::parse("not-a-token"),
            Err(AuthError::MalformedInvitation)
        ));
        let draft = create_pairing_invitation(&identity(), "2026-06-18T10:00:00Z", 100, 10)
            .expect("invitation creates");
        let expired = approve_pairing_invitation(
            &identity(),
            &draft.invitation,
            &draft.token,
            "Laptop",
            "2026-06-18T10:01:00Z",
            111,
        );
        assert!(matches!(expired, Err(AuthError::InvitationExpired { .. })));

        let mut approved = draft.invitation.clone();
        approved.status = "approved".to_string();
        let reused = approve_pairing_invitation(
            &identity(),
            &approved,
            &draft.token,
            "Laptop",
            "2026-06-18T10:01:00Z",
            101,
        );
        assert!(matches!(
            reused,
            Err(AuthError::InvitationNotPending { .. })
        ));
    }

    #[test]
    fn approval_creates_decryptable_key_envelope() {
        let draft = create_pairing_invitation(&identity(), "2026-06-18T10:00:00Z", 100, 600)
            .expect("invitation creates");

        let approval = approve_pairing_invitation(
            &identity(),
            &draft.invitation,
            &draft.token,
            "Travel laptop",
            "2026-06-18T10:01:00Z",
            101,
        )
        .expect("approval works");
        let opened = open_key_envelope(
            &approval.envelope,
            &approval.device.device_key_hex,
            &approval.device.device_id,
        )
        .expect("envelope opens");

        assert_eq!(approval.device.display_name, "Travel laptop");
        assert_eq!(opened, identity().sync_key_hex);
    }

    #[test]
    fn debug_output_redacts_secret_bearing_domain_objects() {
        let identity = identity();
        let draft = create_pairing_invitation(&identity, "2026-06-18T10:00:00Z", 100, 600)
            .expect("invitation creates");
        let token_text = draft.token.expose_for_cli();
        let token_secret = token_text
            .rsplit(':')
            .next()
            .expect("token contains secret")
            .to_string();
        let approval = approve_pairing_invitation(
            &identity,
            &draft.invitation,
            &draft.token,
            "Travel laptop",
            "2026-06-18T10:01:00Z",
            101,
        )
        .expect("approval works");
        let device_key = approval.device.device_key_hex.clone();

        let formatted_identity = format!("{identity:?}");
        let formatted_token = format!("{:?}", draft.token);
        let formatted_draft = format!("{draft:?}");
        let formatted_device = format!("{:?}", approval.device);
        let formatted_approval = format!("{approval:?}");

        assert!(!formatted_identity.contains(&identity.sync_key_hex));
        assert!(!formatted_token.contains(&token_secret));
        assert!(!formatted_draft.contains(&token_secret));
        assert!(!formatted_device.contains(&device_key));
        assert!(!formatted_approval.contains(&device_key));
        assert!(formatted_identity.contains("<redacted>"));
        assert!(formatted_token.contains("<redacted>"));
        assert!(formatted_device.contains("<redacted>"));
    }

    #[test]
    fn recovery_grant_uses_redacted_references_and_idempotent_revocation() {
        let raw_secret = "recovery-secret-should-not-persist";
        let rejected = create_recovery_grant(
            "account-test",
            "device-local",
            raw_secret,
            "laptop recovery",
            "2026-06-18T10:00:00Z",
            100,
            600,
        )
        .expect_err("raw recovery material is rejected");
        assert!(!rejected.to_string().contains(raw_secret));

        let grant = create_recovery_grant(
            "account-test",
            "device-local",
            "recovery-ref:grant-alpha",
            "laptop recovery",
            "2026-06-18T10:00:00Z",
            100,
            600,
        )
        .expect("grant creates");
        assert_eq!(grant.status, "pending");
        assert_eq!(grant.expires_at_unix, 700);

        let consumed =
            consume_recovery_grant(&grant, "2026-06-18T10:01:00Z", 101).expect("grant consumes");
        assert_eq!(consumed.status, "consumed");

        let expired = consume_recovery_grant(&grant, "2026-06-18T10:20:00Z", 700)
            .expect_err("expired grant is rejected");
        assert!(matches!(expired, AuthError::RecoveryGrantExpired { .. }));

        let revoked = revoke_recovery_grant(&grant, "2026-06-18T10:02:00Z").expect("grant revokes");
        let revoked_again =
            revoke_recovery_grant(&revoked, "2026-06-18T10:03:00Z").expect("revocation repeats");
        assert_eq!(revoked_again.revoked_at, revoked.revoked_at);
        assert!(format!("{revoked_again:?}").contains("recovery-ref:grant-alpha"));
    }

    #[test]
    fn device_rotation_intent_records_key_envelope_generation() {
        let intent = create_device_rotation_intent(DeviceRotationIntentInput {
            account_id: "account-test",
            device_id: "device-local",
            requested_by_session_id: Some("session-alpha"),
            reason: "recovery rotation",
            created_at: "2026-06-18T10:00:00Z",
            now_unix: 100,
            ttl_seconds: 600,
            current_generation: 2,
        })
        .expect("intent creates");
        assert_eq!(intent.status, "pending");
        assert_eq!(intent.key_envelope_generation, 2);

        let completed = complete_device_rotation_intent(&intent, "2026-06-18T10:01:00Z", 3)
            .expect("intent completes");
        assert_eq!(completed.status, "completed");
        assert_eq!(completed.key_envelope_generation, 3);

        let repeated = complete_device_rotation_intent(&completed, "2026-06-18T10:02:00Z", 4)
            .expect_err("completed intent cannot complete again");
        assert!(matches!(
            repeated,
            AuthError::RotationIntentNotPending { .. }
        ));

        let raw_reason = "device-key-secret";
        let rejected = create_device_rotation_intent(DeviceRotationIntentInput {
            account_id: "account-test",
            device_id: "device-local",
            requested_by_session_id: None,
            reason: raw_reason,
            created_at: "2026-06-18T10:00:00Z",
            now_unix: 100,
            ttl_seconds: 600,
            current_generation: 0,
        })
        .expect_err("secret-looking reason is rejected");
        assert!(!rejected.to_string().contains(raw_reason));
    }
}

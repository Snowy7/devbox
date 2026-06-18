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
    InvalidHex {
        field: &'static str,
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
    Crypto,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidHex { field } => write!(f, "{field} must be 64 hex characters"),
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

fn hash_secret_hex(secret_hex: &str) -> String {
    blake3::hash(secret_hex.as_bytes()).to_hex().to_string()
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
}

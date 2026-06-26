use crate::auth::{hash_secret, verify_secret};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;

const SESSION_TTL_SECONDS: u64 = 300;
const POLL_INTERVAL_SECONDS: u64 = 2;

#[derive(Debug, Default)]
pub struct PairingStore {
    sessions: HashMap<String, PairingSession>,
}

#[derive(Debug, Clone)]
struct PairingSession {
    session_id: String,
    pairing_code: String,
    device_name: String,
    device_type: String,
    app_id: String,
    app_version: String,
    client_nonce_hash: String,
    created_at: u64,
    expires_at: u64,
    status: StoredPairingStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum StoredPairingStatus {
    Pending,
    Approved { access_token: String },
    Rejected,
}

#[derive(Debug, Deserialize)]
pub struct CreatePairingRequest {
    #[serde(rename = "deviceName")]
    pub device_name: Option<String>,
    #[serde(rename = "deviceType")]
    pub device_type: Option<String>,
    #[serde(rename = "appId")]
    pub app_id: Option<String>,
    #[serde(rename = "appVersion")]
    pub app_version: Option<String>,
    #[serde(rename = "clientNonce")]
    pub client_nonce: String,
}

#[derive(Debug, Serialize)]
pub struct CreatePairingResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "pairingCode")]
    pub pairing_code: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
    #[serde(rename = "pollIntervalSeconds")]
    pub poll_interval_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct PollPairingResponse {
    pub status: &'static str,
    #[serde(rename = "expiresAt", skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(
        rename = "pollIntervalSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    pub poll_interval_seconds: Option<u64>,
    #[serde(rename = "accessToken", skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(rename = "tokenType", skip_serializing_if = "Option::is_none")]
    pub token_type: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct PendingPairingSessionsResponse {
    pub data: Vec<PendingPairingSession>,
}

#[derive(Debug, Serialize)]
pub struct PendingPairingSession {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "pairingCode")]
    pub pairing_code: String,
    #[serde(rename = "deviceName")]
    pub device_name: String,
    #[serde(rename = "deviceType")]
    pub device_type: String,
    #[serde(rename = "appId")]
    pub app_id: String,
    #[serde(rename = "appVersion")]
    pub app_version: String,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "expiresAt")]
    pub expires_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct ApprovePairingRequest {
    #[serde(rename = "pairingCode")]
    pub pairing_code: String,
    #[serde(rename = "deviceLabel")]
    pub device_label: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RejectPairingRequest {
    #[serde(rename = "pairingCode")]
    pub pairing_code: String,
}

#[derive(Debug, Serialize)]
pub struct PairingDecisionResponse {
    pub status: &'static str,
    #[serde(rename = "clientId", skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingApprovalCandidate {
    pub session_id: String,
    pub token_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingError {
    pub status: u16,
    pub code: &'static str,
    pub message: &'static str,
}

impl PairingStore {
    pub fn create(
        &mut self,
        request: CreatePairingRequest,
        now: u64,
    ) -> Result<CreatePairingResponse, PairingError> {
        let client_nonce = request.client_nonce.trim();
        if client_nonce.is_empty() {
            return Err(PairingError::bad_request(
                "missing_client_nonce",
                "clientNonce is required",
            ));
        }

        self.prune_expired(now);
        let session_id = self.unique_session_id()?;
        let pairing_code = self.unique_pairing_code()?;
        let expires_at = now.saturating_add(SESSION_TTL_SECONDS);
        let session = PairingSession {
            session_id: session_id.clone(),
            pairing_code: pairing_code.clone(),
            device_name: clean_label(request.device_name.as_deref())
                .unwrap_or("Android TV".to_string()),
            device_type: clean_label(request.device_type.as_deref())
                .unwrap_or("android_tv".to_string()),
            app_id: clean_label(request.app_id.as_deref()).unwrap_or_default(),
            app_version: clean_label(request.app_version.as_deref()).unwrap_or_default(),
            client_nonce_hash: hash_secret(client_nonce),
            created_at: now,
            expires_at,
            status: StoredPairingStatus::Pending,
        };
        self.sessions.insert(session_id.clone(), session);

        Ok(CreatePairingResponse {
            session_id,
            pairing_code,
            expires_at,
            poll_interval_seconds: POLL_INTERVAL_SECONDS,
        })
    }

    pub fn poll(
        &mut self,
        session_id: &str,
        client_nonce: &str,
        now: u64,
    ) -> Result<PollPairingResponse, PairingError> {
        self.prune_expired(now);
        let Some(session) = self.sessions.get(session_id) else {
            return Ok(PollPairingResponse::expired());
        };
        if !verify_secret(client_nonce, &session.client_nonce_hash) {
            return Err(PairingError::unauthorized(
                "invalid_client_nonce",
                "valid client nonce required",
            ));
        }
        if now > session.expires_at {
            return Ok(PollPairingResponse::expired());
        }

        Ok(match &session.status {
            StoredPairingStatus::Pending => PollPairingResponse {
                status: "pending",
                expires_at: Some(session.expires_at),
                poll_interval_seconds: Some(POLL_INTERVAL_SECONDS),
                access_token: None,
                token_type: None,
            },
            StoredPairingStatus::Approved { access_token } => PollPairingResponse {
                status: "approved",
                expires_at: None,
                poll_interval_seconds: None,
                access_token: Some(access_token.clone()),
                token_type: Some("Bearer"),
            },
            StoredPairingStatus::Rejected => PollPairingResponse {
                status: "rejected",
                expires_at: None,
                poll_interval_seconds: None,
                access_token: None,
                token_type: None,
            },
        })
    }

    pub fn pending_sessions(&mut self, now: u64) -> PendingPairingSessionsResponse {
        self.prune_expired(now);
        let mut data = self
            .sessions
            .values()
            .filter(|session| matches!(session.status, StoredPairingStatus::Pending))
            .map(|session| PendingPairingSession {
                session_id: session.session_id.clone(),
                pairing_code: session.pairing_code.clone(),
                device_name: session.device_name.clone(),
                device_type: session.device_type.clone(),
                app_id: session.app_id.clone(),
                app_version: session.app_version.clone(),
                created_at: session.created_at,
                expires_at: session.expires_at,
            })
            .collect::<Vec<_>>();
        data.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        PendingPairingSessionsResponse { data }
    }

    pub fn approval_candidate(
        &mut self,
        pairing_code: &str,
        device_label: Option<&str>,
        now: u64,
    ) -> Result<PairingApprovalCandidate, PairingError> {
        self.prune_expired(now);
        let session = self
            .sessions
            .values()
            .find(|session| session.pairing_code == normalize_pairing_code(pairing_code))
            .ok_or_else(|| {
                PairingError::not_found("pairing_not_found", "pairing code not found")
            })?;
        if now > session.expires_at {
            return Err(PairingError::not_found(
                "pairing_expired",
                "pairing code expired",
            ));
        }
        if !matches!(session.status, StoredPairingStatus::Pending) {
            return Err(PairingError::conflict(
                "pairing_not_pending",
                "pairing session is not pending",
            ));
        }
        Ok(PairingApprovalCandidate {
            session_id: session.session_id.clone(),
            token_name: clean_label(device_label).unwrap_or_else(|| session.device_name.clone()),
        })
    }

    pub fn approve(&mut self, session_id: &str, access_token: String) -> Result<(), PairingError> {
        let Some(session) = self.sessions.get_mut(session_id) else {
            return Err(PairingError::not_found(
                "pairing_not_found",
                "pairing session not found",
            ));
        };
        session.status = StoredPairingStatus::Approved { access_token };
        Ok(())
    }

    pub fn reject(&mut self, pairing_code: &str, now: u64) -> Result<(), PairingError> {
        self.prune_expired(now);
        let normalized = normalize_pairing_code(pairing_code);
        let session = self
            .sessions
            .values_mut()
            .find(|session| session.pairing_code == normalized)
            .ok_or_else(|| {
                PairingError::not_found("pairing_not_found", "pairing code not found")
            })?;
        session.status = StoredPairingStatus::Rejected;
        Ok(())
    }

    fn prune_expired(&mut self, now: u64) {
        self.sessions.retain(|_, session| now <= session.expires_at);
    }

    fn unique_session_id(&self) -> Result<String, PairingError> {
        for _ in 0..8 {
            let id = format!("ps_{}", random_hex(16)?);
            if !self.sessions.contains_key(&id) {
                return Ok(id);
            }
        }
        Err(PairingError::internal(
            "pairing_generation_failed",
            "failed to generate pairing session",
        ))
    }

    fn unique_pairing_code(&self) -> Result<String, PairingError> {
        for _ in 0..16 {
            let code = random_pairing_code()?;
            let in_use = self.sessions.values().any(|session| {
                session.pairing_code == code
                    && matches!(session.status, StoredPairingStatus::Pending)
            });
            if !in_use {
                return Ok(code);
            }
        }
        Err(PairingError::internal(
            "pairing_generation_failed",
            "failed to generate pairing code",
        ))
    }
}

impl PollPairingResponse {
    fn expired() -> Self {
        Self {
            status: "expired",
            expires_at: None,
            poll_interval_seconds: None,
            access_token: None,
            token_type: None,
        }
    }
}

impl PairingError {
    fn bad_request(code: &'static str, message: &'static str) -> Self {
        Self {
            status: 400,
            code,
            message,
        }
    }

    fn unauthorized(code: &'static str, message: &'static str) -> Self {
        Self {
            status: 401,
            code,
            message,
        }
    }

    fn not_found(code: &'static str, message: &'static str) -> Self {
        Self {
            status: 404,
            code,
            message,
        }
    }

    fn conflict(code: &'static str, message: &'static str) -> Self {
        Self {
            status: 409,
            code,
            message,
        }
    }

    fn internal(code: &'static str, message: &'static str) -> Self {
        Self {
            status: 500,
            code,
            message,
        }
    }
}

fn clean_label(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(80).collect())
}

fn normalize_pairing_code(value: &str) -> String {
    value.chars().filter(|c| c.is_ascii_digit()).collect()
}

fn random_pairing_code() -> Result<String, PairingError> {
    let mut bytes = [0u8; 4];
    fill_random(&mut bytes)?;
    let value = u32::from_be_bytes(bytes) % 1_000_000;
    Ok(format!("{value:06}"))
}

fn random_hex(len: usize) -> Result<String, PairingError> {
    let mut bytes = vec![0u8; len];
    fill_random(&mut bytes)?;
    Ok(to_hex(&bytes))
}

fn fill_random(bytes: &mut [u8]) -> Result<(), PairingError> {
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .map_err(|_| PairingError::internal("random_unavailable", "random source unavailable"))
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> CreatePairingRequest {
        CreatePairingRequest {
            device_name: Some("Living Room ATV".into()),
            device_type: Some("android_tv".into()),
            app_id: Some("com.example.atv".into()),
            app_version: Some("1.0".into()),
            client_nonce: "nonce".into(),
        }
    }

    #[test]
    fn creates_and_lists_pending_session_with_code() {
        let mut store = PairingStore::default();
        let created = store.create(request(), 100).unwrap();

        let pending = store.pending_sessions(100);
        assert_eq!(1, pending.data.len());
        assert_eq!(created.pairing_code, pending.data[0].pairing_code);
        assert_eq!("Living Room ATV", pending.data[0].device_name);
        assert_eq!(400, created.expires_at);
    }

    #[test]
    fn approve_by_code_returns_token_on_poll() {
        let mut store = PairingStore::default();
        let created = store.create(request(), 100).unwrap();
        let candidate = store
            .approval_candidate(&created.pairing_code, Some("Den TV"), 101)
            .unwrap();
        assert_eq!("Den TV", candidate.token_name);

        store
            .approve(&candidate.session_id, "token".into())
            .unwrap();
        let polled = store.poll(&created.session_id, "nonce", 102).unwrap();
        assert_eq!("approved", polled.status);
        assert_eq!(Some("token".to_string()), polled.access_token);
    }

    #[test]
    fn rejects_invalid_nonce() {
        let mut store = PairingStore::default();
        let created = store.create(request(), 100).unwrap();
        let err = store.poll(&created.session_id, "wrong", 101).unwrap_err();
        assert_eq!(401, err.status);
    }

    #[test]
    fn expired_sessions_are_not_listed() {
        let mut store = PairingStore::default();
        store.create(request(), 100).unwrap();
        assert!(store.pending_sessions(401).data.is_empty());
    }
}

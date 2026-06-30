use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientToken {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub hash: String,
    pub created_at: u64,
    pub last_seen_at: Option<u64>,
    pub enabled: bool,
}

pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("sha256:{}", to_hex(&hasher.finalize()))
}

pub fn verify_secret(secret: &str, expected_hash: &str) -> bool {
    constant_time_eq(hash_secret(secret).as_bytes(), expected_hash.as_bytes())
}

pub fn find_valid_token<'a>(tokens: &'a mut [ClientToken], raw: &str) -> Option<&'a ClientToken> {
    let now = now_secs();
    let hash = hash_secret(raw);
    for token in tokens.iter_mut() {
        if token.enabled && constant_time_eq(hash.as_bytes(), token.hash.as_bytes()) {
            token.last_seen_at = Some(now);
            return Some(token);
        }
    }
    None
}

pub fn generate_token(name: &str) -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!("atv_{}_{}", sanitize_name(name), to_hex(&bytes)))
}

pub fn generate_token_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!("tok_{}", to_hex(&bytes)))
}

pub fn normalize_token_ids(tokens: &mut [ClientToken]) {
    let mut used = HashSet::new();
    for token in tokens.iter_mut() {
        if token.id.trim().is_empty() {
            token.id = legacy_token_id(&token.hash);
        }
        if used.insert(token.id.clone()) {
            continue;
        }
        let base = token.id.clone();
        let mut suffix = 2;
        loop {
            let candidate = format!("{base}_{suffix}");
            if used.insert(candidate.clone()) {
                token.id = candidate;
                break;
            }
            suffix += 1;
        }
    }
}

fn legacy_token_id(hash: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(hash.as_bytes());
    let hex = to_hex(&hasher.finalize());
    format!("legacy_{}", &hex[..16])
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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

    #[test]
    fn verifies_hash() {
        let hash = hash_secret("secret");
        assert!(verify_secret("secret", &hash));
        assert!(!verify_secret("wrong", &hash));
    }

    #[test]
    fn valid_token_updates_last_seen() {
        let raw = "token";
        let mut tokens = vec![ClientToken {
            id: "token-id".into(),
            name: "tv".into(),
            hash: hash_secret(raw),
            created_at: 1,
            last_seen_at: None,
            enabled: true,
        }];
        assert_eq!("tv", find_valid_token(&mut tokens, raw).unwrap().name);
        assert!(tokens[0].last_seen_at.is_some());
    }

    #[test]
    fn normalizes_missing_and_duplicate_token_ids() {
        let hash = hash_secret("token");
        let mut tokens = vec![
            ClientToken {
                id: String::new(),
                name: "tv-a".into(),
                hash: hash.clone(),
                created_at: 1,
                last_seen_at: None,
                enabled: true,
            },
            ClientToken {
                id: String::new(),
                name: "tv-b".into(),
                hash,
                created_at: 2,
                last_seen_at: None,
                enabled: true,
            },
        ];

        normalize_token_ids(&mut tokens);

        assert!(tokens[0].id.starts_with("legacy_"));
        assert!(tokens[1].id.starts_with("legacy_"));
        assert_ne!(tokens[0].id, tokens[1].id);
    }
}

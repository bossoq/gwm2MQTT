use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;

pub(crate) const CRED_FILE: &str = "/opt/gwm2mqtt/credentials.json";

const MACHINE_ID_PATH: &str = "/etc/machine-id";
const FALLBACK_KEY_FILE: &str = "/opt/gwm2mqtt/.secret";
const APP_SALT: &str = "gwm2mqtt-creds-v1";

/// Returns a 32-byte AES key derived from a machine-specific identifier.
/// Primary source: /etc/machine-id (Linux/systemd, stable across reboots).
/// Fallback: a random 32-byte key generated once and persisted to FALLBACK_KEY_FILE.
fn derive_key() -> [u8; 32] {
    let seed = fs::read_to_string(MACHINE_ID_PATH)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| {
            fs::read_to_string(FALLBACK_KEY_FILE)
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|_| {
                    let bytes: [u8; 32] = rand::random();
                    let hex: String = bytes.iter().fold(String::with_capacity(64), |mut acc, b| { use std::fmt::Write; let _ = write!(acc, "{b:02x}"); acc });
                    let _ = fs::write(FALLBACK_KEY_FILE, &hex);
                    hex
                })
        });

    let mut hasher = Sha256::new();
    hasher.update(format!("{APP_SALT}:{seed}").as_bytes());
    hasher.finalize().into()
}

fn encrypt(plaintext: &str) -> Result<String, String> {
    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("Cipher init failed: {e}"))?;

    let nonce_bytes: [u8; 12] = rand::random();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| format!("Encryption failed: {e}"))?;

    // Layout: 12-byte nonce || ciphertext+tag, base64-encoded
    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(B64.encode(&combined))
}

fn decrypt(encoded: &str) -> Result<String, String> {
    let combined = B64
        .decode(encoded)
        .map_err(|e| format!("Base64 decode failed: {e}"))?;
    if combined.len() < 13 {
        return Err("Ciphertext too short".to_string());
    }

    let key = derive_key();
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("Cipher init failed: {e}"))?;

    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("Decryption failed: {e}"))?;

    String::from_utf8(plaintext).map_err(|e| format!("UTF-8 decode failed: {e}"))
}

/// Reads and decrypts the credentials file.
/// Transparently handles legacy plaintext JSON for one-time migration.
pub fn read_cred_json() -> Result<Value, String> {
    let content = fs::read_to_string(CRED_FILE).map_err(|_| {
        "No credentials found. Set EMAIL/PASSWORD at build time or save via the dashboard."
            .to_string()
    })?;

    let envelope: Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse credentials file: {e}"))?;

    // Encrypted envelope: {"v":1,"data":"<base64>"}
    if envelope.get("v") == Some(&json!(1)) {
        if let Some(data) = envelope["data"].as_str() {
            let plaintext = decrypt(data)?;
            return serde_json::from_str::<Value>(&plaintext)
                .map_err(|e| format!("Failed to parse decrypted credentials: {e}"));
        }
    }

    // Legacy plaintext JSON — return as-is; will be re-encrypted on next write
    Ok(envelope)
}

/// Encrypts and writes credentials to the credentials file.
pub fn write_cred_json(value: &Value) -> Result<(), String> {
    let plaintext = serde_json::to_string(value).map_err(|e| format!("JSON encode failed: {e}"))?;
    let encrypted = encrypt(&plaintext)?;
    let envelope = json!({"v": 1, "data": encrypted});
    let content = serde_json::to_string_pretty(&envelope)
        .map_err(|e| format!("Envelope encode failed: {e}"))?;
    fs::write(CRED_FILE, content).map_err(|e| format!("Write failed: {e}"))
}

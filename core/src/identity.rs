//! Persistent node identity (Ed25519 secret key).
//!
//! A stable identity means the same device keeps the same public key across
//! restarts, which makes tickets/relays predictable and lets transfers resume.

use std::path::Path;

use iroh::SecretKey;

use crate::error::Result;

/// Load the persisted identity at `key_path`, or generate and persist a new one.
pub fn load_or_create(key_path: &Path) -> Result<SecretKey> {
    match std::fs::read(key_path) {
        Ok(bytes) if bytes.len() == 32 => {
            let arr: [u8; 32] = bytes.try_into().expect("length checked above");
            Ok(SecretKey::from_bytes(&arr))
        }
        _ => {
            // VERIFY (ARCHITECTURE.md §13): `SecretKey::generate()` signature on
            // iroh 1.0. If it requires an RNG, use `SecretKey::generate(rand::rngs::OsRng)`.
            let sk = SecretKey::generate();
            std::fs::write(key_path, sk.to_bytes())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(sk)
        }
    }
}

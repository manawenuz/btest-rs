use md5::{Digest, Md5};
use rand::RngCore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::{self, BtestError, Result, AUTH_FAILED, AUTH_OK, AUTH_REQUIRED};

pub fn generate_challenge() -> [u8; 16] {
    let mut nonce = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut nonce);
    nonce
}

/// Compute the double-MD5 response: MD5(password + MD5(password + challenge))
pub fn compute_auth_hash(password: &str, challenge: &[u8; 16]) -> [u8; 16] {
    // hash1 = MD5(password + challenge)
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(challenge);
    let hash1 = hasher.finalize();

    // hash2 = MD5(password + hash1)
    let mut hasher = Md5::new();
    hasher.update(password.as_bytes());
    hasher.update(&hash1);
    hasher.finalize().into()
}

/// Server-side: send auth challenge and verify response.
/// Returns Ok(()) if auth succeeds or no auth is configured.
pub async fn server_authenticate<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut S,
    username: Option<&str>,
    password: Option<&str>,
) -> Result<()> {
    match (username, password) {
        (None, None) => {
            // No auth required
            stream.write_all(&AUTH_OK).await?;
            stream.flush().await?;
            Ok(())
        }
        (_, Some(pass)) => {
            // Send auth challenge
            stream.write_all(&AUTH_REQUIRED).await?;
            let challenge = generate_challenge();
            stream.write_all(&challenge).await?;
            stream.flush().await?;

            // Receive response: 16 bytes hash + 32 bytes username
            let mut response = [0u8; 48];
            stream.read_exact(&mut response).await?;

            let received_hash = &response[0..16];
            let received_user = &response[16..48];

            // Extract username (null-terminated)
            let user_end = received_user
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(32);
            let received_username = std::str::from_utf8(&received_user[..user_end])
                .unwrap_or("");

            // Verify username if configured
            if let Some(expected_user) = username {
                if received_username != expected_user {
                    tracing::warn!("Auth failed: username mismatch (got '{}')", received_username);
                    stream.write_all(&AUTH_FAILED).await?;
                    stream.flush().await?;
                    return Err(BtestError::AuthFailed);
                }
            }

            // Verify hash
            let expected_hash = compute_auth_hash(pass, &challenge);
            if received_hash != expected_hash {
                tracing::warn!("Auth failed: hash mismatch for user '{}'", received_username);
                stream.write_all(&AUTH_FAILED).await?;
                stream.flush().await?;
                return Err(BtestError::AuthFailed);
            }

            tracing::info!("Auth successful for user '{}'", received_username);
            stream.write_all(&AUTH_OK).await?;
            stream.flush().await?;
            Ok(())
        }
        (Some(_), None) => {
            // Username but no password - treat as no auth
            stream.write_all(&AUTH_OK).await?;
            stream.flush().await?;
            Ok(())
        }
    }
}

/// Client-side: respond to auth challenge if required.
pub async fn client_authenticate<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut S,
    resp: [u8; 4],
    username: &str,
    password: &str,
) -> Result<()> {
    if resp == AUTH_OK {
        return Ok(());
    }

    if resp == AUTH_REQUIRED {
        // Read 16-byte challenge
        let mut challenge = [0u8; 16];
        stream.read_exact(&mut challenge).await?;

        // Compute response
        let hash = compute_auth_hash(password, &challenge);

        // Build 48-byte response: 16 hash + 32 username
        let mut response = [0u8; 48];
        response[0..16].copy_from_slice(&hash);
        let user_bytes = username.as_bytes();
        let copy_len = user_bytes.len().min(32);
        response[16..16 + copy_len].copy_from_slice(&user_bytes[..copy_len]);

        stream.write_all(&response).await?;
        stream.flush().await?;

        // Read auth result
        let result = protocol::recv_response(stream).await?;
        if result == AUTH_OK {
            tracing::info!("Authentication successful");
            Ok(())
        } else {
            Err(BtestError::AuthFailed)
        }
    } else if resp == [0x03, 0x00, 0x00, 0x00] {
        Err(BtestError::Protocol(
            "EC-SRP5 authentication (RouterOS >= 6.43) is not supported".into(),
        ))
    } else {
        Err(BtestError::Protocol(format!(
            "Unexpected auth response: {:02x?}",
            resp
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_hash_known_vector() {
        // From the Perl reference: password "test", challenge as hex "ad32d6f94d28161625f2f390bb895637"
        let challenge: [u8; 16] = [
            0xad, 0x32, 0xd6, 0xf9, 0x4d, 0x28, 0x16, 0x16, 0x25, 0xf2, 0xf3, 0x90, 0xbb, 0x89,
            0x56, 0x37,
        ];
        let hash = compute_auth_hash("test", &challenge);
        let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, "3c968565bc0314f281a6da1571cf7255");
    }

    #[test]
    fn test_empty_password() {
        let challenge = generate_challenge();
        let hash = compute_auth_hash("", &challenge);
        assert_eq!(hash.len(), 16);
    }
}

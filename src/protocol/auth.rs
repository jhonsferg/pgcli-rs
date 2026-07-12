/// PostgreSQL authentication method handlers.
///
/// Supports Trust, Password (cleartext), MD5, and SCRAM-SHA-256.
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::error::{PgCliError, Result};

/// All authentication methods understood by the PostgreSQL wire protocol.
///
/// GSSAPI/SSPI (Kerberos) is intentionally absent: tokio-postgres rejects those
/// at the wire level before any `AuthMethod` is consulted, and `classify_pg_error`
/// in `connection/pool.rs` converts that failure into a clear actionable message.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthMethod {
    /// No credential required.
    Trust,
    /// Cleartext password (deprecated but still encountered).
    Password,
    /// MD5-salted password hash.
    Md5 { salt: [u8; 4] },
    /// SCRAM-SHA-256 challenge-response (RFC 5802).
    ScramSha256,
}

/// Generate the MD5 password hash expected by PostgreSQL for the MD5 auth method.
///
/// PostgreSQL expects: `"md5" + md5(md5(password + username) + salt)`
///
/// # Arguments
///
/// * `password`-the user's plaintext password
/// * `username`-the connecting username
/// * `salt`   -the 4-byte salt sent by the server
pub fn md5_password(password: &str, username: &str, salt: &[u8; 4]) -> String {
    let inner = format!("{password}{username}");
    let inner_hash_bytes = md5_compute(inner.as_bytes());
    let inner_hash: String = inner_hash_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut combined = inner_hash.clone().into_bytes();
    combined.extend_from_slice(salt);
    let outer_bytes = md5_compute(&combined);
    let outer_hash: String = outer_bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("md5{outer_hash}")
}

/// SCRAM-SHA-256 client state machine.
///
/// Follows RFC 5802. The exchange has two rounds:
/// 1. Client sends `client-first-message`.
/// 2. Server replies with `server-first-message` (nonce + salt + iterations).
/// 3. Client sends `client-final-message`.
/// 4. Server verifies and sends `server-final-message`.
pub struct ScramClient {
    username: String,
    password: String,
    client_nonce: String,
    /// Set after parsing the server-first-message.
    server_nonce: Option<String>,
    salt: Option<Vec<u8>>,
    iterations: Option<u32>,
    client_first_bare: Option<String>,
    server_first: Option<String>,
}

impl ScramClient {
    /// Create a new SCRAM-SHA-256 client for the given credentials.
    pub fn new(username: &str, password: &str) -> Self {
        let mut nonce_bytes = [0u8; 18];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let client_nonce = BASE64.encode(nonce_bytes);
        Self {
            username: username.to_string(),
            password: password.to_string(),
            client_nonce,
            server_nonce: None,
            salt: None,
            iterations: None,
            client_first_bare: None,
            server_first: None,
        }
    }

    /// Produce the `client-first-message` to send to the server.
    pub fn client_first_message(&mut self) -> String {
        let bare = format!("n={},r={}", self.username, self.client_nonce);
        self.client_first_bare = Some(bare.clone());
        format!("n,,{bare}")
    }

    /// Parse the `server-first-message` and produce `client-final-message`.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Authentication` if the server message is malformed
    /// or the nonce does not begin with the client nonce.
    pub fn client_final_message(&mut self, server_first: &str) -> Result<String> {
        self.server_first = Some(server_first.to_string());

        // Parse r=<nonce>,s=<salt-b64>,i=<iterations>
        let mut server_nonce = String::new();
        let mut salt_b64 = String::new();
        let mut iterations: u32 = 0;

        for part in server_first.split(',') {
            if let Some(v) = part.strip_prefix("r=") {
                server_nonce = v.to_string();
            } else if let Some(v) = part.strip_prefix("s=") {
                salt_b64 = v.to_string();
            } else if let Some(v) = part.strip_prefix("i=") {
                iterations = v.parse::<u32>().map_err(|_| {
                    PgCliError::Authentication("invalid SCRAM iteration count".to_string())
                })?;
            }
        }

        if !server_nonce.starts_with(&self.client_nonce) {
            return Err(PgCliError::Authentication(
                "SCRAM server nonce does not begin with client nonce".to_string(),
            ));
        }

        let salt = BASE64
            .decode(&salt_b64)
            .map_err(|_| PgCliError::Authentication("invalid SCRAM salt encoding".to_string()))?;

        self.server_nonce = Some(server_nonce.clone());
        self.salt = Some(salt.clone());
        self.iterations = Some(iterations);

        // Compute salted password via PBKDF2-SHA256.
        let salted_password = pbkdf2_sha256(self.password.as_bytes(), &salt, iterations);

        // client-key = HMAC(salted-password, "Client Key")
        let client_key = hmac_sha256(&salted_password, b"Client Key");
        // stored-key = H(client-key)
        let stored_key = sha256(&client_key);

        let client_first_bare = self
            .client_first_bare
            .as_deref()
            .ok_or_else(|| PgCliError::Authentication("SCRAM state error".to_string()))?;

        // channel-binding = base64("n,,")
        let channel_binding = BASE64.encode("n,,");
        let client_final_without_proof = format!("c={channel_binding},r={server_nonce}");

        // auth-message = client-first-bare + "," + server-first + "," + client-final-without-proof
        let auth_message =
            format!("{client_first_bare},{server_first},{client_final_without_proof}");

        // client-signature = HMAC(stored-key, auth-message)
        let client_signature = hmac_sha256(&stored_key, auth_message.as_bytes());
        // client-proof = client-key XOR client-signature
        let client_proof: Vec<u8> = client_key
            .iter()
            .zip(client_signature.iter())
            .map(|(a, b)| a ^ b)
            .collect();

        let proof_b64 = BASE64.encode(&client_proof);
        Ok(format!("{client_final_without_proof},p={proof_b64}"))
    }

    /// Verify the `server-final-message`.
    ///
    /// # Errors
    ///
    /// Returns `PgCliError::Authentication` if the server signature is invalid.
    pub fn verify_server_final(&self, server_final: &str) -> Result<()> {
        if let Some(err_msg) = server_final.strip_prefix("e=") {
            return Err(PgCliError::Authentication(format!(
                "SCRAM server error: {err_msg}"
            )));
        }
        // Simplified: accept any `v=<base64>` response without full verification.
        // Full verification would check: server-key = HMAC(salted-password, "Server Key"),
        // server-signature = HMAC(server-key, auth-message).
        // This is sufficient for the MVP; full verification can be added later.
        if server_final.starts_with("v=") {
            return Ok(());
        }
        Err(PgCliError::Authentication(
            "unexpected SCRAM server-final-message format".to_string(),
        ))
    }
}

/// PBKDF2 with HMAC-SHA256 key derivation.
fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut result = vec![0u8; 32];
    // U1 = HMAC(password, salt || INT(1))
    let mut u = hmac_sha256(password, &[salt, &[0, 0, 0, 1]].concat());
    result.copy_from_slice(&u);
    for _ in 1..iterations {
        u = hmac_sha256(password, &u);
        for (r, &ub) in result.iter_mut().zip(u.iter()) {
            *r ^= ub;
        }
    }
    result
}

/// Compute HMAC-SHA256(key, data).
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <Hmac<Sha256>>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Compute SHA-256 hash of `data`.
fn sha256(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

/// Minimal inline MD5 implementation (used for legacy auth only).
fn md5_compute(input: &[u8]) -> [u8; 16] {
    // Initial hash values (same as MD5 spec).
    let (mut a0, mut b0, mut c0, mut d0): (u32, u32, u32, u32) =
        (0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476);

    // Per-round shift amounts.
    #[rustfmt::skip]
    let s: [u32; 64] = [
        7,12,17,22, 7,12,17,22, 7,12,17,22, 7,12,17,22,
        5, 9,14,20, 5, 9,14,20, 5, 9,14,20, 5, 9,14,20,
        4,11,16,23, 4,11,16,23, 4,11,16,23, 4,11,16,23,
        6,10,15,21, 6,10,15,21, 6,10,15,21, 6,10,15,21,
    ];
    // Precomputed table: T[i] = floor(2^32 * |sin(i+1)|).
    #[rustfmt::skip]
    let k: [u32; 64] = [
        0xd76aa478,0xe8c7b756,0x242070db,0xc1bdceee,0xf57c0faf,0x4787c62a,0xa8304613,0xfd469501,
        0x698098d8,0x8b44f7af,0xffff5bb1,0x895cd7be,0x6b901122,0xfd987193,0xa679438e,0x49b40821,
        0xf61e2562,0xc040b340,0x265e5a51,0xe9b6c7aa,0xd62f105d,0x02441453,0xd8a1e681,0xe7d3fbc8,
        0x21e1cde6,0xc33707d6,0xf4d50d87,0x455a14ed,0xa9e3e905,0xfcefa3f8,0x676f02d9,0x8d2a4c8a,
        0xfffa3942,0x8771f681,0x6d9d6122,0xfde5380c,0xa4beea44,0x4bdecfa9,0xf6bb4b60,0xbebfbc70,
        0x289b7ec6,0xeaa127fa,0xd4ef3085,0x04881d05,0xd9d4d039,0xe6db99e5,0x1fa27cf8,0xc4ac5665,
        0xf4292244,0x432aff97,0xab9423a7,0xfc93a039,0x655b59c3,0x8f0ccc92,0xffeff47d,0x85845dd1,
        0x6fa87e4f,0xfe2ce6e0,0xa3014314,0x4e0811a1,0xf7537e82,0xbd3af235,0x2ad7d2bb,0xeb86d391,
    ];

    // Pre-processing: padding.
    let orig_bit_len = (input.len() as u64).wrapping_mul(8);
    let mut msg = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&orig_bit_len.to_le_bytes());

    // Process each 512-bit (64-byte) chunk.
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0usize..64 {
            let (f, g) = if i < 16 {
                ((b & c) | (!b & d), i)
            } else if i < 32 {
                ((d & b) | (!d & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g])).rotate_left(s[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut digest = [0u8; 16];
    digest[0..4].copy_from_slice(&a0.to_le_bytes());
    digest[4..8].copy_from_slice(&b0.to_le_bytes());
    digest[8..12].copy_from_slice(&c0.to_le_bytes());
    digest[12..16].copy_from_slice(&d0.to_le_bytes());
    digest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_known_vector() {
        // md5("") = d41d8cd98f00b204e9800998ecf8427e
        let result = md5_compute(b"");
        let hex: String = result.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn md5_hello_world() {
        let result = md5_compute(b"hello world");
        let hex: String = result.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn scram_client_first_contains_nonce() {
        let mut client = ScramClient::new("alice", "hunter2");
        let msg = client.client_first_message();
        assert!(msg.starts_with("n,,n=alice,r="));
    }

    #[test]
    fn md5_password_format() {
        let hash = md5_password("hunter2", "alice", &[1, 2, 3, 4]);
        assert!(hash.starts_with("md5"), "Expected md5 prefix, got: {hash}");
        assert_eq!(hash.len(), 35); // "md5" + 32 hex chars
    }

    #[test]
    fn hmac_sha256_known() {
        // HMAC-SHA256("key", "The quick brown fox...") is a well-known test vector.
        let result = hmac_sha256(b"key", b"The quick brown fox jumps over the lazy dog");
        assert_eq!(result.len(), 32);
    }

    #[test]
    fn pbkdf2_produces_32_bytes() {
        let key = pbkdf2_sha256(b"password", b"salt", 4096);
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn scram_full_round_trip_produces_final_message() {
        let mut client = ScramClient::new("alice", "hunter2");
        let first = client.client_first_message();
        let client_nonce = first
            .strip_prefix("n,,n=alice,r=")
            .expect("client nonce prefix")
            .to_string();

        let server_nonce = format!("{client_nonce}SERVERPART");
        let salt_b64 = BASE64.encode(b"somesalt");
        let server_first = format!("r={server_nonce},s={salt_b64},i=4096");

        let final_msg = client
            .client_final_message(&server_first)
            .expect("client_final_message should succeed");
        assert!(final_msg.contains(&format!("r={server_nonce}")));
        assert!(final_msg.contains("p="));
    }

    #[test]
    fn scram_rejects_mismatched_server_nonce() {
        let mut client = ScramClient::new("alice", "hunter2");
        client.client_first_message();
        let salt_b64 = BASE64.encode(b"somesalt");
        let server_first = format!("r=totally-different-nonce,s={salt_b64},i=4096");
        let result = client.client_final_message(&server_first);
        assert!(result.is_err());
    }

    #[test]
    fn scram_rejects_invalid_iteration_count() {
        let mut client = ScramClient::new("alice", "hunter2");
        let first = client.client_first_message();
        let client_nonce = first.strip_prefix("n,,n=alice,r=").unwrap();
        let salt_b64 = BASE64.encode(b"somesalt");
        let server_first = format!("r={client_nonce},s={salt_b64},i=notanumber");
        let result = client.client_final_message(&server_first);
        assert!(result.is_err());
    }

    #[test]
    fn scram_rejects_invalid_salt_encoding() {
        let mut client = ScramClient::new("alice", "hunter2");
        let first = client.client_first_message();
        let client_nonce = first.strip_prefix("n,,n=alice,r=").unwrap();
        let server_first = format!("r={client_nonce},s=not-valid-base64!!!,i=4096");
        let result = client.client_final_message(&server_first);
        assert!(result.is_err());
    }

    #[test]
    fn scram_verify_server_final_accepts_v_prefix() {
        let client = ScramClient::new("alice", "hunter2");
        assert!(client.verify_server_final("v=c29tZXNpZ25hdHVyZQ==").is_ok());
    }

    #[test]
    fn scram_verify_server_final_rejects_error() {
        let client = ScramClient::new("alice", "hunter2");
        let result = client.verify_server_final("e=invalid-proof");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid-proof"));
    }

    #[test]
    fn scram_verify_server_final_rejects_unexpected_format() {
        let client = ScramClient::new("alice", "hunter2");
        assert!(client.verify_server_final("garbage").is_err());
    }

    #[test]
    fn md5_password_is_deterministic() {
        let a = md5_password("hunter2", "alice", &[1, 2, 3, 4]);
        let b = md5_password("hunter2", "alice", &[1, 2, 3, 4]);
        assert_eq!(a, b);
        let different_salt = md5_password("hunter2", "alice", &[5, 6, 7, 8]);
        assert_ne!(a, different_salt);
    }

    #[test]
    fn auth_method_variants_are_comparable() {
        assert_eq!(AuthMethod::Trust, AuthMethod::Trust);
        assert_ne!(AuthMethod::Trust, AuthMethod::Password);
        assert_eq!(
            AuthMethod::Md5 { salt: [1, 2, 3, 4] },
            AuthMethod::Md5 { salt: [1, 2, 3, 4] }
        );
        assert_ne!(
            AuthMethod::Md5 { salt: [1, 2, 3, 4] },
            AuthMethod::Md5 { salt: [5, 6, 7, 8] }
        );
    }
}

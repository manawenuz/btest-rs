//! EC-SRP5 authentication for MikroTik RouterOS >= 6.43.
//!
//! Implements the Curve25519-Weierstrass EC-SRP5 protocol used by MikroTik btest.
//! Based on research by Margin Research (Apache-2.0 License):
//! https://github.com/MarginResearch/mikrotik_authentication
//!
//! btest framing: `[len:1][payload]` (no 0x06 handler byte, unlike Winbox).

use std::sync::LazyLock;

use num_bigint::BigUint;
use num_integer::Integer;
use num_traits::{One, Zero};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::protocol::{BtestError, Result};

// --- Curve25519 parameters in Weierstrass form (cached, computed once) ---

static P: LazyLock<BigUint> = LazyLock::new(|| {
    BigUint::parse_bytes(
        b"7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffed",
        16,
    )
    .unwrap()
});

static CURVE_ORDER: LazyLock<BigUint> = LazyLock::new(|| {
    BigUint::parse_bytes(
        b"1000000000000000000000000000000014def9dea2f79cd65812631a5cf5d3ed",
        16,
    )
    .unwrap()
});

static WEIERSTRASS_A: LazyLock<BigUint> = LazyLock::new(|| {
    BigUint::parse_bytes(
        b"2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa984914a144",
        16,
    )
    .unwrap()
});

const MONT_A: u64 = 486662;

// --- Modular arithmetic ---

fn modinv(a: &BigUint, modulus: &BigUint) -> BigUint {
    // Fermat's little theorem: a^(p-2) mod p
    let exp = modulus - BigUint::from(2u32);
    a.modpow(&exp, modulus)
}

fn legendre_symbol(a: &BigUint, p: &BigUint) -> i32 {
    let exp = (p - BigUint::one()) / BigUint::from(2u32);
    let l = a.modpow(&exp, p);
    if l == p - BigUint::one() {
        -1
    } else if l == BigUint::zero() {
        0
    } else {
        1
    }
}

fn prime_mod_sqrt(a: &BigUint, p_val: &BigUint) -> Option<(BigUint, BigUint)> {
    let a = a % p_val;
    if a.is_zero() {
        return Some((BigUint::zero(), BigUint::zero()));
    }
    if legendre_symbol(&a, p_val) != 1 {
        return None;
    }

    // For p ≡ 5 (mod 8) — which is Curve25519's case — use Atkin's algorithm
    // This is more reliable than Tonelli-Shanks for this specific case
    let p_mod_8 = p_val % BigUint::from(8u32);
    if p_mod_8 == BigUint::from(5u32) {
        // v = (2a)^((p-5)/8) mod p
        let exp = (p_val - BigUint::from(5u32)) / BigUint::from(8u32);
        let two_a = (BigUint::from(2u32) * &a) % p_val;
        let v = two_a.modpow(&exp, p_val);
        // i = 2 * a * v^2 mod p
        let i_val = (BigUint::from(2u32) * &a % p_val * &v % p_val * &v) % p_val;
        // x = a * v * (i - 1) mod p
        let i_minus_1 = if i_val >= BigUint::one() {
            (&i_val - BigUint::one()) % p_val
        } else {
            (p_val - BigUint::one() + &i_val) % p_val
        };
        let x = (&a * &v % p_val * &i_minus_1) % p_val;
        // Verify: x^2 ≡ a (mod p)
        let check = (&x * &x) % p_val;
        if check == a {
            let other = p_val - &x;
            return Some((x, other));
        }
        return None;
    }

    if p_mod_8 == BigUint::from(3u32) || p_mod_8 == BigUint::from(7u32) {
        let exp = (p_val + BigUint::one()) / BigUint::from(4u32);
        let x = a.modpow(&exp, p_val);
        let other = p_val - &x;
        return Some((x, other));
    }

    // General Tonelli-Shanks for other primes
    let mut q = p_val - BigUint::one();
    let mut s = 0u32;
    while q.is_even() {
        s += 1;
        q >>= 1;
    }

    let mut z = BigUint::from(2u32);
    while legendre_symbol(&z, p_val) != -1 {
        z += BigUint::one();
    }
    let mut c = z.modpow(&q, p_val);
    let mut x = a.modpow(&((&q + BigUint::one()) / BigUint::from(2u32)), p_val);
    let mut t = a.modpow(&q, p_val);
    let mut m = s;

    while t != BigUint::one() {
        let mut i = 1u32;
        let mut tmp = (&t * &t) % p_val;
        while tmp != BigUint::one() {
            tmp = (&tmp * &tmp) % p_val;
            i += 1;
        }
        let b = c.modpow(&BigUint::from(1u32 << (m - i - 1)), p_val);
        x = (&x * &b) % p_val;
        t = ((&t * &b % p_val) * &b) % p_val;
        c = (&b * &b) % p_val;
        m = i;
    }

    let other = p_val - &x;
    Some((x, other))
}

// --- Weierstrass curve point ---

#[derive(Clone, Debug)]
struct Point {
    x: BigUint,
    y: BigUint,
    infinity: bool,
}

impl Point {
    fn infinity() -> Self {
        Self {
            x: BigUint::zero(),
            y: BigUint::zero(),
            infinity: true,
        }
    }

    fn new(x: BigUint, y: BigUint) -> Self {
        Self {
            x,
            y,
            infinity: false,
        }
    }

    fn add(&self, other: &Point) -> Point {
        let p_val = &*P;
        if self.infinity {
            return other.clone();
        }
        if other.infinity {
            return self.clone();
        }
        if self.x == other.x && self.y != other.y {
            return Point::infinity();
        }

        let lam = if self.x == other.x && self.y == other.y {
            // Point doubling
            let three_x_sq = (BigUint::from(3u32) * &self.x * &self.x + &*WEIERSTRASS_A) % p_val;
            let two_y = (BigUint::from(2u32) * &self.y) % p_val;
            (three_x_sq * modinv(&two_y, p_val)) % p_val
        } else {
            // Point addition
            let dy = if other.y >= self.y {
                (&other.y - &self.y) % p_val
            } else {
                (p_val - (&self.y - &other.y) % p_val) % p_val
            };
            let dx = if other.x >= self.x {
                (&other.x - &self.x) % p_val
            } else {
                (p_val - (&self.x - &other.x) % p_val) % p_val
            };
            (dy * modinv(&dx, p_val)) % p_val
        };

        let x3 = {
            let lam_sq = (&lam * &lam) % p_val;
            let sum_x = (&self.x + &other.x) % p_val;
            if lam_sq >= sum_x {
                (lam_sq - sum_x) % p_val
            } else {
                (p_val - (sum_x - lam_sq) % p_val) % p_val
            }
        };
        let y3 = {
            let dx = if self.x >= x3 {
                (&self.x - &x3) % p_val
            } else {
                (p_val - (&x3 - &self.x) % p_val) % p_val
            };
            let prod = (&lam * dx) % p_val;
            if prod >= self.y {
                (prod - &self.y) % p_val
            } else {
                (p_val - (&self.y - prod) % p_val) % p_val
            }
        };

        Point::new(x3, y3)
    }

    fn scalar_mul(&self, scalar: &BigUint) -> Point {
        let mut result = Point::infinity();
        let mut base = self.clone();
        let bits = scalar.bits();

        for i in 0..bits {
            if scalar.bit(i) {
                result = result.add(&base);
            }
            base = base.add(&base);
        }
        result
    }
}

// --- WCurve: Curve25519 in Weierstrass form ---

struct WCurve {
    g: Point,
    conversion_from_m: BigUint,
    conversion_to_m: BigUint,
}

impl WCurve {
    fn new() -> Self {
        let p_val = &*P;
        let mont_a = BigUint::from(MONT_A);
        let three_inv = modinv(&BigUint::from(3u32), p_val);
        let conversion_from_m = (&mont_a * &three_inv) % p_val;
        let conversion_to_m = (p_val - &conversion_from_m) % p_val;

        let mut curve = WCurve {
            g: Point::infinity(),
            conversion_from_m,
            conversion_to_m,
        };
        curve.g = curve.lift_x(&BigUint::from(9u32), false);
        curve
    }

    fn to_montgomery(&self, pt: &Point) -> ([u8; 32], u8) {
        let p_val = &*P;
        let x = (&pt.x + &self.conversion_to_m) % p_val;
        let parity = if pt.y.bit(0) { 1u8 } else { 0u8 };
        let mut bytes = [0u8; 32];
        let x_bytes = x.to_bytes_be();
        let start = 32 - x_bytes.len().min(32);
        bytes[start..].copy_from_slice(&x_bytes[..x_bytes.len().min(32)]);
        (bytes, parity)
    }

    fn lift_x(&self, x_mont: &BigUint, parity: bool) -> Point {
        let p_val = &*P;
        let x = x_mont % p_val;
        // y^2 = x^3 + Ax^2 + x (Montgomery)
        let y_squared = (&x * &x * &x + BigUint::from(MONT_A) * &x * &x + &x) % p_val;
        // Convert x to Weierstrass
        let x_w = (&x + &self.conversion_from_m) % p_val;

        if let Some((y1, y2)) = prime_mod_sqrt(&y_squared, p_val) {
            let pt1 = Point::new(x_w.clone(), y1);
            let pt2 = Point::new(x_w, y2);
            if parity {
                if pt1.y.bit(0) { pt1 } else { pt2 }
            } else {
                if !pt1.y.bit(0) { pt1 } else { pt2 }
            }
        } else {
            Point::infinity()
        }
    }

    fn gen_public_key(&self, priv_key: &[u8; 32]) -> ([u8; 32], u8) {
        let scalar = BigUint::from_bytes_be(priv_key);
        let pt = self.g.scalar_mul(&scalar);
        self.to_montgomery(&pt)
    }

    fn redp1(&self, x_bytes: &[u8; 32], parity: bool) -> Point {
        let mut x = sha256_bytes(x_bytes);
        loop {
            let x2 = sha256_bytes(&x);
            let x_int = BigUint::from_bytes_be(&x2);
            let pt = self.lift_x(&x_int, parity);
            if !pt.infinity {
                return pt;
            }
            let mut val = BigUint::from_bytes_be(&x);
            val += BigUint::one();
            x = bigint_to_32bytes(&val);
        }
    }

    fn gen_password_validator_priv(
        &self,
        username: &str,
        password: &str,
        salt: &[u8; 16],
    ) -> [u8; 32] {
        let inner = sha256_bytes(format!("{}:{}", username, password).as_bytes());
        let mut input = Vec::with_capacity(16 + 32);
        input.extend_from_slice(salt);
        input.extend_from_slice(&inner);
        sha256_bytes(&input)
    }
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn bigint_to_32bytes(val: &BigUint) -> [u8; 32] {
    let bytes = val.to_bytes_be();
    let mut out = [0u8; 32];
    let start = 32usize.saturating_sub(bytes.len());
    let copy_len = bytes.len().min(32);
    out[start..start + copy_len].copy_from_slice(&bytes[bytes.len() - copy_len..]);
    out
}

// --- EC-SRP5 Client Authentication ---

/// Perform EC-SRP5 authentication as a client.
/// Called after receiving `03 00 00 00` from the server.
pub async fn client_authenticate<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut S,
    username: &str,
    password: &str,
) -> Result<()> {
    tracing::info!("Starting EC-SRP5 authentication");
    let w = WCurve::new();

    // Generate client ephemeral keypair
    let s_a: [u8; 32] = rand::random();
    let (x_w_a, x_w_a_parity) = w.gen_public_key(&s_a);

    // MSG1: [len][username\0][pubkey:32][parity:1]
    let mut payload = Vec::new();
    payload.extend_from_slice(username.as_bytes());
    payload.push(0x00);
    payload.extend_from_slice(&x_w_a);
    payload.push(x_w_a_parity);
    let mut msg1 = vec![payload.len() as u8];
    msg1.extend_from_slice(&payload);
    stream.write_all(&msg1).await?;
    stream.flush().await?;
    tracing::debug!("EC-SRP5: sent client pubkey ({} bytes)", msg1.len());

    // MSG2: [len][server_pubkey:32][parity:1][salt:16]
    let mut resp_header = [0u8; 1];
    stream.read_exact(&mut resp_header).await?;
    let resp_len = resp_header[0] as usize;
    let mut resp_data = vec![0u8; resp_len];
    stream.read_exact(&mut resp_data).await?;

    if resp_data.len() < 49 {
        return Err(BtestError::Protocol(format!(
            "EC-SRP5: server challenge too short ({} bytes)",
            resp_data.len()
        )));
    }

    let mut x_w_b = [0u8; 32];
    x_w_b.copy_from_slice(&resp_data[0..32]);
    let x_w_b_parity = resp_data[32] != 0;
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&resp_data[33..49]);

    tracing::debug!("EC-SRP5: received server challenge (salt={})", hex::encode(&salt));

    // Compute shared secret
    let i = w.gen_password_validator_priv(username, password, &salt);
    let (x_gamma, _) = w.gen_public_key(&i);
    let v = w.redp1(&x_gamma, true);

    let w_b_point = w.lift_x(&BigUint::from_bytes_be(&x_w_b), x_w_b_parity);
    let w_b_unblinded = w_b_point.add(&v);

    let mut j_input = Vec::with_capacity(64);
    j_input.extend_from_slice(&x_w_a);
    j_input.extend_from_slice(&x_w_b);
    let j = sha256_bytes(&j_input);

    let i_int = BigUint::from_bytes_be(&i);
    let j_int = BigUint::from_bytes_be(&j);
    let s_a_int = BigUint::from_bytes_be(&s_a);
    let order = &*CURVE_ORDER;
    let scalar = ((&i_int * &j_int) + &s_a_int) % order;

    let z_point = w_b_unblinded.scalar_mul(&scalar);
    let (z, _) = w.to_montgomery(&z_point);

    // MSG3: [len][client_cc:32]
    let mut cc_input = Vec::with_capacity(64);
    cc_input.extend_from_slice(&j);
    cc_input.extend_from_slice(&z);
    let client_cc = sha256_bytes(&cc_input);

    let mut msg3 = vec![client_cc.len() as u8];
    msg3.extend_from_slice(&client_cc);
    stream.write_all(&msg3).await?;
    stream.flush().await?;
    tracing::debug!("EC-SRP5: sent client proof");

    // MSG4: [len][server_cc:32]
    let mut resp4_header = [0u8; 1];
    stream.read_exact(&mut resp4_header).await?;
    let resp4_len = resp4_header[0] as usize;
    let mut server_cc_received = vec![0u8; resp4_len];
    stream.read_exact(&mut server_cc_received).await?;

    // Verify server confirmation
    let mut sc_input = Vec::with_capacity(96);
    sc_input.extend_from_slice(&j);
    sc_input.extend_from_slice(&client_cc);
    sc_input.extend_from_slice(&z);
    let server_cc_expected = sha256_bytes(&sc_input);

    if server_cc_received == server_cc_expected {
        tracing::info!("EC-SRP5 authentication successful");
        Ok(())
    } else {
        // Check if server sent an error message
        if let Ok(msg) = std::str::from_utf8(&server_cc_received) {
            Err(BtestError::Protocol(format!(
                "EC-SRP5 authentication failed: {}",
                msg
            )))
        } else {
            Err(BtestError::AuthFailed)
        }
    }
}

// --- EC-SRP5 Server Authentication ---

/// Server-side EC-SRP5 credential store.
pub struct EcSrp5Credentials {
    salt: [u8; 16],
    x_gamma: [u8; 32],
    gamma_parity: bool,
}

impl EcSrp5Credentials {
    /// Derive EC-SRP5 credentials from username/password (done once at startup).
    pub fn derive(username: &str, password: &str) -> Self {
        let salt: [u8; 16] = rand::random();
        let w = WCurve::new();
        let i = w.gen_password_validator_priv(username, password, &salt);
        let (x_gamma, parity) = w.gen_public_key(&i);
        Self {
            salt,
            x_gamma,
            gamma_parity: parity != 0,
        }
    }
}

/// Perform EC-SRP5 authentication as a server.
/// Called after sending `03 00 00 00` to the client.
pub async fn server_authenticate<S: AsyncReadExt + AsyncWriteExt + Unpin>(
    stream: &mut S,
    username: &str,
    creds: &EcSrp5Credentials,
) -> Result<()> {
    tracing::info!("Starting EC-SRP5 server authentication");
    let w = WCurve::new();

    // MSG1: read [len][username\0][pubkey:32][parity:1]
    let mut len_buf = [0u8; 1];
    stream.read_exact(&mut len_buf).await?;
    let msg_len = len_buf[0] as usize;
    let mut msg1_data = vec![0u8; msg_len];
    stream.read_exact(&mut msg1_data).await?;

    // Parse username
    let null_pos = msg1_data.iter().position(|&b| b == 0)
        .ok_or_else(|| BtestError::Protocol("EC-SRP5: no null terminator in username".into()))?;
    let client_username = std::str::from_utf8(&msg1_data[..null_pos])
        .map_err(|_| BtestError::Protocol("EC-SRP5: invalid username encoding".into()))?;

    if client_username != username {
        tracing::warn!("EC-SRP5: username mismatch (got '{}')", client_username);
        return Err(BtestError::AuthFailed);
    }

    let key_start = null_pos + 1;
    if msg1_data.len() < key_start + 33 {
        return Err(BtestError::Protocol("EC-SRP5: client message too short".into()));
    }
    let mut x_w_a = [0u8; 32];
    x_w_a.copy_from_slice(&msg1_data[key_start..key_start + 32]);
    let x_w_a_parity = msg1_data[key_start + 32] != 0;

    tracing::debug!("EC-SRP5: received client pubkey from '{}'", client_username);

    // Generate server ephemeral keypair
    let s_b: [u8; 32] = rand::random();
    let s_b_int = BigUint::from_bytes_be(&s_b);
    let pub_b = w.g.scalar_mul(&s_b_int);

    // Compute password-entangled public key: W_b = s_b*G + redp1(x_gamma, 0)
    let gamma = w.redp1(&creds.x_gamma, false);
    let w_b = pub_b.add(&gamma);
    let (x_w_b, x_w_b_parity) = w.to_montgomery(&w_b);

    // MSG2: [len][server_pubkey:32][parity:1][salt:16]
    let mut payload2 = Vec::with_capacity(49);
    payload2.extend_from_slice(&x_w_b);
    payload2.push(x_w_b_parity);
    payload2.extend_from_slice(&creds.salt);
    let mut msg2 = vec![payload2.len() as u8];
    msg2.extend_from_slice(&payload2);
    stream.write_all(&msg2).await?;
    stream.flush().await?;
    tracing::debug!("EC-SRP5: sent server challenge");

    // Compute shared secret (server side: ECPESVDP-SRP-B)
    let mut j_input = Vec::with_capacity(64);
    j_input.extend_from_slice(&x_w_a);
    j_input.extend_from_slice(&x_w_b);
    let j = sha256_bytes(&j_input);
    let j_int = BigUint::from_bytes_be(&j);

    // Server ECPESVDP-SRP-B: Z = s_b * (W_a + j * gamma)
    // gamma = lift_x(x_gamma, parity=1) — the raw validator public key point
    // (NOT redp1 — that's used for blinding W_b, not for verification)
    let w_a = w.lift_x(&BigUint::from_bytes_be(&x_w_a), x_w_a_parity);
    let gamma = w.lift_x(&BigUint::from_bytes_be(&creds.x_gamma), creds.gamma_parity);
    let j_gamma = gamma.scalar_mul(&j_int);
    let sum = w_a.add(&j_gamma);
    let z_point = sum.scalar_mul(&s_b_int);
    let (z, _) = w.to_montgomery(&z_point);

    // MSG3: read [len][client_cc:32]
    let mut len3 = [0u8; 1];
    stream.read_exact(&mut len3).await?;
    let mut client_cc = vec![0u8; len3[0] as usize];
    stream.read_exact(&mut client_cc).await?;

    // Verify client confirmation
    let mut cc_input = Vec::with_capacity(64);
    cc_input.extend_from_slice(&j);
    cc_input.extend_from_slice(&z);
    let expected_cc = sha256_bytes(&cc_input);

    if client_cc != expected_cc {
        tracing::warn!("EC-SRP5: client proof mismatch");
        return Err(BtestError::AuthFailed);
    }

    // MSG4: [len][server_cc:32]
    let mut sc_input = Vec::with_capacity(96);
    sc_input.extend_from_slice(&j);
    sc_input.extend_from_slice(&client_cc);
    sc_input.extend_from_slice(&z);
    let server_cc = sha256_bytes(&sc_input);

    let mut msg4 = vec![server_cc.len() as u8];
    msg4.extend_from_slice(&server_cc);
    stream.write_all(&msg4).await?;
    stream.flush().await?;

    tracing::info!("EC-SRP5 server authentication successful for '{}'", client_username);
    Ok(())
}

mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_curve_generator() {
        let w = WCurve::new();
        assert!(!w.g.infinity);
        // Generator from lift_x(9, false) should produce a valid point
        let (x_mont, _) = w.to_montgomery(&w.g);
        let x_int = BigUint::from_bytes_be(&x_mont);
        assert_eq!(x_int, BigUint::from(9u32));
    }

    #[test]
    fn test_pubkey_generation() {
        let w = WCurve::new();
        let priv_key = [1u8; 32];
        let (pubkey, parity) = w.gen_public_key(&priv_key);
        assert_ne!(pubkey, [0u8; 32]);
        assert!(parity <= 1);
    }

    #[test]
    fn test_password_validator() {
        let w = WCurve::new();
        let salt = [0x01u8, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08,
                    0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10];
        let i = w.gen_password_validator_priv("testuser", "testpass", &salt);
        assert_ne!(i, [0u8; 32]);
        // Deterministic: same inputs produce same output
        let i2 = w.gen_password_validator_priv("testuser", "testpass", &salt);
        assert_eq!(i, i2);
        // Different password produces different result
        let i3 = w.gen_password_validator_priv("testuser", "other", &salt);
        assert_ne!(i, i3);
    }

    #[test]
    fn test_redp1() {
        let w = WCurve::new();
        let input = [42u8; 32];
        let pt = w.redp1(&input, false);
        assert!(!pt.infinity);
    }

    #[test]
    fn test_scalar_mul_identity() {
        let w = WCurve::new();
        let one = BigUint::one();
        let pt = w.g.scalar_mul(&one);
        assert_eq!(pt.x, w.g.x);
        assert_eq!(pt.y, w.g.y);
    }
}

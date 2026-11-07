//! Port of `packages/ai/src/auth/oauth/pkce.ts`.
//! PORT-STATUS: reviewed
//!
//! `crypto.getRandomValues` is `rand`'s thread RNG and `crypto.subtle.digest` is `sha2`. TS returns an anonymous
//! `{ verifier, challenge }` object; the port returns `Pkce`.
//! PKCE utilities using Web Crypto API.
//! Works in both Node.js 20+ and browsers.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Encode bytes as base64url string.
fn base64url_encode(bytes: &[u8]) -> String {
	URL_SAFE_NO_PAD.encode(bytes)
}

/// The result of `generatePKCE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
	pub verifier: String,
	pub challenge: String,
}

/// Generate PKCE code verifier and challenge.
/// Uses Web Crypto API for cross-platform compatibility.
pub async fn generate_pkce() -> Pkce {
	// Generate random verifier
	let mut verifier_bytes = [0u8; 32];
	rand::rng().fill_bytes(&mut verifier_bytes);
	let verifier = base64url_encode(&verifier_bytes);

	// Compute SHA-256 challenge
	let hash = Sha256::digest(verifier.as_bytes());
	let challenge = base64url_encode(&hash);

	Pkce { verifier, challenge }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn challenge_is_the_base64url_sha256_of_the_verifier() {
		let pkce = futures::executor::block_on(generate_pkce());
		assert_eq!(pkce.verifier.len(), 43);
		assert!(!pkce.verifier.contains(['+', '/', '=']));
		assert_eq!(
			pkce.challenge,
			base64url_encode(&Sha256::digest(pkce.verifier.as_bytes()))
		);
		assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
	}
}

//! Port of `packages/ai/src/utils/hash.ts`.
//! PORT-STATUS: reviewed

/// Fast deterministic hash to shorten long strings
pub fn short_hash(text: &str) -> String {
	let mut h1: u32 = 0xdeadbeef;
	let mut h2: u32 = 0x41c6ce57;
	for ch in text.encode_utf16() {
		h1 = (h1 ^ u32::from(ch)).wrapping_mul(2654435761);
		h2 = (h2 ^ u32::from(ch)).wrapping_mul(1597334677);
	}
	h1 = (h1 ^ (h1 >> 16)).wrapping_mul(2246822507) ^ (h2 ^ (h2 >> 13)).wrapping_mul(3266489909);
	h2 = (h2 ^ (h2 >> 16)).wrapping_mul(2246822507) ^ (h1 ^ (h1 >> 13)).wrapping_mul(3266489909);
	format!("{}{}", to_base36(h2), to_base36(h1))
}

fn to_base36(mut value: u32) -> String {
	const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
	let mut digits = Vec::new();
	loop {
		digits.push(DIGITS[(value % 36) as usize]);
		value /= 36;
		if value == 0 {
			break;
		}
	}
	digits.reverse();
	String::from_utf8(digits).expect("base-36 digits are ASCII")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn matches_the_typescript_hash() {
		let hashes: Vec<String> = ["", "a", "hello world", "😀é"].into_iter().map(short_hash).collect();
		assert_eq!(
			hashes,
			["k4n83c7h0j2b", "m8735310ae7sx", "n7rb4n1m39uz8", "lbk9325860g2"]
		);
	}
}

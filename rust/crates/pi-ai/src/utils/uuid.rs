//! Port of `packages/ai/src/utils/uuid.ts`.
//! PORT-STATUS: ported
//!
//! The generator state is per thread, like the per-isolate module state in TS. Random bytes come from `rand`, where
//! TS calls `crypto.getRandomValues`.

use std::cell::Cell;

use pi_host::{PiError, PiResult};
use rand::RngCore;

const MAX_UUID_V7_TIMESTAMP: f64 = 281_474_976_710_655.0;
const MAX_SEQUENCE: u64 = (1 << 41) - 1;

thread_local! {
	static LAST_ORDINARY_TIMESTAMP: Cell<f64> = const { Cell::new(-1.0) };
	static SEQUENCE: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Generate a time-ordered UUIDv7. A supplied timestamp is preserved for follower ids.
pub fn uuidv7(timestamp_ms: Option<f64>) -> PiResult<String> {
	let requested_timestamp = timestamp_ms.unwrap_or_else(pi_host::now_ms);
	if requested_timestamp.fract() != 0.0
		|| !requested_timestamp.is_finite()
		|| requested_timestamp < 0.0
		|| requested_timestamp > MAX_UUID_V7_TIMESTAMP
	{
		return Err(PiError::named(
			"RangeError",
			"UUIDv7 timestamp must be an integer between 0 and 281474976710655",
		));
	}

	let effective_timestamp = match timestamp_ms {
		None => {
			let timestamp = requested_timestamp.max(LAST_ORDINARY_TIMESTAMP.get());
			LAST_ORDINARY_TIMESTAMP.set(timestamp);
			timestamp
		}
		Some(timestamp) => timestamp,
	};

	let mut bytes = [0u8; 16];
	rand::rng().fill_bytes(&mut bytes);
	let sequence = match SEQUENCE.get() {
		None => {
			(u64::from(bytes[1]) << 32)
				| (u64::from(bytes[2]) << 24)
				| (u64::from(bytes[3]) << 16)
				| (u64::from(bytes[4]) << 8)
				| u64::from(bytes[5])
		}
		Some(MAX_SEQUENCE) => return Err(PiError::named("RangeError", "UUIDv7 generator sequence exhausted")),
		Some(sequence) => sequence + 1,
	};
	SEQUENCE.set(Some(sequence));

	let timestamp = effective_timestamp as u64;
	for index in (0..=5).rev() {
		bytes[index] = (timestamp >> ((5 - index) * 8)) as u8;
	}
	bytes[6] = 0x70 | ((sequence >> 37) & 0x0f) as u8;
	bytes[7] = ((sequence >> 29) & 0xff) as u8;
	bytes[8] = 0x80 | ((sequence >> 23) & 0x3f) as u8;
	bytes[9] = ((sequence >> 15) & 0xff) as u8;
	bytes[10] = ((sequence >> 7) & 0xff) as u8;
	bytes[11] = (((sequence & 0x7f) << 1) as u8) | (bytes[11] & 0x01);

	let hex = hex::encode(bytes);
	Ok(format!(
		"{}-{}-{}-{}-{}",
		&hex[0..8],
		&hex[8..12],
		&hex[12..16],
		&hex[16..20],
		&hex[20..]
	))
}

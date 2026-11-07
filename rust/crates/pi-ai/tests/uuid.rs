//! Port of `packages/ai/test/uuid.test.ts`.
//!
//! The clock is `TestHost`'s, which starts at 1,700,000,000,000 ms and only moves forward, so the TS clock rollback
//! becomes follower ids with an earlier timestamp between ordinary ones. The fresh-randomness test stubs
//! `crypto.getRandomValues`, which has no Rust counterpart; it checks that two ids with the same timestamp differ in
//! their random tail instead.

use pi_ai::utils::uuid::uuidv7;

const TIMESTAMP: f64 = 1_700_000_000_000.0;

fn is_uuid_v7(uuid: &str) -> bool {
	regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
		.unwrap()
		.is_match(uuid)
}

fn parse_timestamp(uuid: &str) -> f64 {
	u64::from_str_radix(&uuid.replace('-', "")[..12], 16).unwrap() as f64
}

#[test]
fn generates_ordered_uuidv7s_while_preserving_follower_timestamps() {
	pi_host::testing::run(|host| async move {
		let first = uuidv7(None).unwrap();
		let second = uuidv7(None).unwrap();
		let follower_timestamp = TIMESTAMP - 1_000.0;
		let followers = [
			uuidv7(Some(follower_timestamp)).unwrap(),
			uuidv7(Some(follower_timestamp)).unwrap(),
		];
		let after_followers = uuidv7(None).unwrap();
		host.advance(1.0);
		let after_advance = uuidv7(None).unwrap();
		let ordinary_ids = [first, second, after_followers, after_advance];

		for id in ordinary_ids.iter().chain(&followers) {
			assert!(is_uuid_v7(id), "{id}");
		}
		let mut sorted = ordinary_ids.clone();
		sorted.sort();
		assert_eq!(sorted, ordinary_ids);
		assert_eq!(
			ordinary_ids.iter().map(|id| parse_timestamp(id)).collect::<Vec<_>>(),
			[TIMESTAMP, TIMESTAMP, TIMESTAMP, TIMESTAMP + 1.0]
		);
		assert_eq!(
			followers.iter().map(|id| parse_timestamp(id)).collect::<Vec<_>>(),
			[follower_timestamp, follower_timestamp]
		);
		assert_ne!(followers[0], followers[1]);
	});
}

#[test]
fn uses_fresh_randomness_for_every_uuid_tail() {
	let first = uuidv7(Some(TIMESTAMP)).unwrap();
	let second = uuidv7(Some(TIMESTAMP)).unwrap();
	assert_ne!(first[first.len() - 8..], second[second.len() - 8..]);
}

#[test]
fn accepts_timestamp_boundaries() {
	for timestamp in [0.0, 2f64.powi(48) - 1.0] {
		assert_eq!(parse_timestamp(&uuidv7(Some(timestamp)).unwrap()), timestamp);
	}
}

#[test]
fn rejects_invalid_timestamps() {
	for timestamp in [-1.0, 2f64.powi(48), 1.5, f64::NAN, f64::INFINITY] {
		let error = uuidv7(Some(timestamp)).unwrap_err();
		assert_eq!(error.name, "RangeError");
		assert_eq!(
			error.message,
			"UUIDv7 timestamp must be an integer between 0 and 281474976710655"
		);
	}
}

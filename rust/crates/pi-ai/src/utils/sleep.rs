//! Port of `packages/ai/src/utils/sleep.ts`.
//! PORT-STATUS: ported
//!
//! The delay is clamped the way `setTimeout` clamps it, which `pi_host::sleep` does not do.

use pi_host::{AbortSignal, PiResult};

pub async fn sleep(ms: f64, signal: &AbortSignal) -> PiResult<()> {
	pi_host::sleep(set_timeout_delay(ms), Some(signal)).await
}

/// Node and browsers wait 1 ms for a delay below 1, above 2^31 - 1, or `NaN`.
fn set_timeout_delay(ms: f64) -> f64 {
	if (1.0..=2_147_483_647.0).contains(&ms) { ms } else { 1.0 }
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn clamps_delays_like_set_timeout() {
		let elapsed = pi_host::testing::run(|_| async {
			let start = pi_host::now_ms();
			let signal = pi_host::AbortController::new().signal();
			for ms in [f64::INFINITY, f64::NAN, -5.0, 0.0, 2_147_483_648.0] {
				sleep(ms, &signal).await.unwrap();
			}
			let clamped = pi_host::now_ms() - start;
			sleep(250.0, &signal).await.unwrap();
			(clamped, pi_host::now_ms() - start)
		});
		assert_eq!(elapsed, (5.0, 255.0));
	}
}

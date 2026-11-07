//! Port of `packages/ai/src/utils/sleep.ts`.
//! PORT-STATUS: ported

use pi_host::{AbortSignal, PiResult};

pub async fn sleep(ms: f64, signal: &AbortSignal) -> PiResult<()> {
	pi_host::sleep(ms, Some(signal)).await
}

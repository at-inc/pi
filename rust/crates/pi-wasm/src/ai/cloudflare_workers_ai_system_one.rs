//! Exports of `api/cloudflare-workers-ai-system-one` and `api/cloudflare-workers-ai-system-one.lazy`.

use pi_ai::api::cloudflare_workers_ai_system_one_lazy;
use wasm_bindgen::prelude::*;

use super::models::NativeClassifier;

/// `cloudflareWorkersAISystemOneApi()`: the module as lazily loaded `ProviderClassifier`; the facade's `classify`
/// calls it too.
#[wasm_bindgen(js_name = aiCloudflareWorkersAISystemOneApi)]
pub fn api() -> NativeClassifier {
	NativeClassifier::new(cloudflare_workers_ai_system_one_lazy::cloudflare_workers_ai_system_one_api())
}

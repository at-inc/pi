//! Exports of `api/typesafe-system-one` and `api/typesafe-system-one.lazy`.

use pi_ai::api::typesafe_system_one_lazy;
use wasm_bindgen::prelude::*;

use super::models::NativeClassifier;

/// `typesafeSystemOneApi()`: the module as lazily loaded `ProviderClassifier`; the facade's `classify` calls it too.
#[wasm_bindgen(js_name = aiTypesafeSystemOneApi)]
pub fn api() -> NativeClassifier {
	NativeClassifier::new(typesafe_system_one_lazy::typesafe_system_one_api())
}

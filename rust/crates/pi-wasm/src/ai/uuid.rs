//! Exports of `utils/uuid`.

use js_sys::{Reflect, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::options::throw;

/// `uuidv7(timestampMs)`. The random bytes come from `globalThis.crypto.getRandomValues` at each call, as in the TS.
#[wasm_bindgen(js_name = aiUuidv7)]
pub fn uuidv7(timestamp_ms: Option<f64>) -> Result<String, JsValue> {
	let crypto = Reflect::get(&js_sys::global(), &"crypto".into())?;
	let get_random_values: js_sys::Function = Reflect::get(&crypto, &"getRandomValues".into())?.dyn_into()?;
	let array = Uint8Array::new_with_length(16);
	get_random_values.call1(&crypto, &array)?;
	let mut bytes = [0u8; 16];
	array.copy_to(&mut bytes);
	throw(pi_ai::utils::uuid::uuidv7_with_random_bytes(timestamp_ms, bytes))
}

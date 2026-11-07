//! Plain data across the boundary: serde values to JS values and back with `serde-wasm-bindgen`, using the Rust
//! serde shapes, which match the TS JSON. Maps become plain objects and `None`/`null` become `null`.

use pi_host::{PiError, PiResult};
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;

const SERIALIZER: serde_wasm_bindgen::Serializer = serde_wasm_bindgen::Serializer::new()
	.serialize_maps_as_objects(true)
	.serialize_missing_as_null(true);

/// The JS value of a serde value.
pub fn to_js<T: Serialize + ?Sized>(value: &T) -> PiResult<JsValue> {
	value
		.serialize(&SERIALIZER)
		.map_err(|error| PiError::type_error(error.to_string()))
}

/// A serde value from a JS value. JS `undefined` and `null` both read as `null`/`None`; functions are rejected, so
/// callers strip them from objects that carry callbacks (see `options`).
pub fn from_js<T: DeserializeOwned>(value: JsValue) -> PiResult<T> {
	serde_wasm_bindgen::from_value(value).map_err(|error| PiError::type_error(error.to_string()))
}

//! Exports of `utils/string-enum`.

use pi_ai::utils::string_enum::{self, StringEnumOptions};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

use crate::json::to_js_text;
use crate::options::{arg, throw};

/// `StringEnum(values, options)`.
#[wasm_bindgen(js_name = aiStringEnum)]
pub fn string_enum(values: JsValue, options: JsValue) -> Result<JsValue, JsValue> {
	let values: Vec<String> = arg(values)?;
	let options: Option<Options> = arg(options)?;
	let options = options.map(|options| StringEnumOptions {
		description: options.description,
		default: options.default,
	});
	let values: Vec<&str> = values.iter().map(String::as_str).collect();
	throw(to_js_text(&string_enum::string_enum(&values, options.as_ref())))
}

#[derive(Deserialize)]
struct Options {
	description: Option<String>,
	default: Option<String>,
}

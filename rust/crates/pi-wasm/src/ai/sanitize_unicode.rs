//! Exports of `utils/sanitize-unicode`. A Rust string cannot hold an unpaired surrogate (`pi_ai`'s
//! `sanitize_surrogates` has nothing to remove), so the export works on the JS string's UTF-16 code units.

use js_sys::JsString;
use wasm_bindgen::prelude::*;

/// `sanitizeSurrogates(text)`: `text` itself when it has no unpaired surrogate, otherwise `text` without them.
#[wasm_bindgen(js_name = aiSanitizeSurrogates)]
pub fn sanitize_surrogates(text: JsString) -> JsString {
	if text.is_well_formed() {
		return text;
	}
	let units: Vec<u16> = text.iter().collect();
	let mut kept = Vec::with_capacity(units.len());
	let mut index = 0;
	while index < units.len() {
		let unit = units[index];
		let paired = (0xd800..=0xdbff).contains(&unit)
			&& units
				.get(index + 1)
				.is_some_and(|next| (0xdc00..=0xdfff).contains(next));
		if paired {
			kept.extend_from_slice(&units[index..index + 2]);
			index += 2;
			continue;
		}
		if !(0xd800..=0xdfff).contains(&unit) {
			kept.push(unit);
		}
		index += 1;
	}
	String::from_utf16(&kept)
		.expect("only paired surrogates are kept")
		.into()
}

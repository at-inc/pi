//! Exports of `auth/oauth/pkce`.

use js_sys::{Object, Promise, Reflect};
use wasm_bindgen::prelude::*;

/// `generatePKCE()`: `{ verifier, challenge }`.
#[wasm_bindgen(js_name = aiGeneratePKCE)]
pub fn generate_pkce() -> Promise {
	let pkce = pi_ai::auth::oauth::pkce::generate_pkce();
	wasm_bindgen_futures::future_to_promise(async move {
		let pkce = pkce.await;
		let object = Object::new();
		Reflect::set(&object, &"verifier".into(), &pkce.verifier.into())?;
		Reflect::set(&object, &"challenge".into(), &pkce.challenge.into())?;
		Ok(object.into())
	})
}

//! Exports of `utils/provider-env`.

use pi_ai::ProviderEnv;
use wasm_bindgen::prelude::*;

use crate::json::from_js_text;
use crate::options::throw;

/// `getProviderEnvValue(name, env)`: `env`, then the host's environment.
#[wasm_bindgen(js_name = aiGetProviderEnvValue)]
pub fn get_provider_env_value(name: &str, env: JsValue) -> Result<Option<String>, JsValue> {
	let env: Option<ProviderEnv> = throw(from_js_text(&env))?;
	Ok(pi_ai::utils::provider_env::get_provider_env_value(name, env.as_ref()))
}

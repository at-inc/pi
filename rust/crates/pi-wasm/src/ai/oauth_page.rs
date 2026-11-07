//! Exports of `utils/oauth-page`.

use pi_ai::utils::oauth_page;
use wasm_bindgen::prelude::*;

/// `oauthSuccessHtml(message)`.
#[wasm_bindgen(js_name = aiOauthSuccessHtml)]
pub fn oauth_success_html(message: &str) -> String {
	oauth_page::oauth_success_html(message)
}

/// `oauthErrorHtml(message, details)`.
#[wasm_bindgen(js_name = aiOauthErrorHtml)]
pub fn oauth_error_html(message: &str, details: Option<String>) -> String {
	oauth_page::oauth_error_html(message, details.as_deref())
}

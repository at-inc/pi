//! Port of `packages/ai/src/compat/extension-oauth-types.ts`.
//! PORT-STATUS: ported
//!
//! `OAuthLoginCallbacks` is a struct of `Rc` callbacks; optional methods are `Option` fields.

use std::rc::Rc;

use pi_host::{AbortSignal, LocalBoxFuture, PiResult};
use serde::{Deserialize, Serialize};

pub use crate::auth::types::OAuthCredentials;

/// Legacy extension OAuth prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthPrompt {
	pub message: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub placeholder: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_empty: Option<bool>,
}

/// Legacy extension OAuth authorization link.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthAuthInfo {
	pub url: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub instructions: Option<String>,
}

/// Legacy extension OAuth device-code notification.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthDeviceCodeInfo {
	pub user_code: String,
	pub verification_uri: String,
	/// Offer best-effort browser opening on user confirmation. Defaults to true; clients may ignore it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub open_browser_on_confirm: Option<bool>,
	/// Offer copying of userCode on user confirmation, replacing the clipboard. Defaults to true.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub copy_code_on_confirm: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub interval_seconds: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub expires_in_seconds: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthSelectOption {
	pub id: String,
	pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthSelectPrompt {
	pub message: String,
	pub options: Vec<OAuthSelectOption>,
}

/// `OAuthLoginCallbacks.onAuth`.
pub type OnAuthFunction = Rc<dyn Fn(OAuthAuthInfo)>;
/// `OAuthLoginCallbacks.onDeviceCode`.
pub type OnDeviceCodeFunction = Rc<dyn Fn(OAuthDeviceCodeInfo)>;
/// `OAuthLoginCallbacks.onPrompt`.
pub type OnPromptFunction = Rc<dyn Fn(OAuthPrompt) -> LocalBoxFuture<'static, PiResult<String>>>;
/// `OAuthLoginCallbacks.onProgress`.
pub type OnProgressFunction = Rc<dyn Fn(String)>;
/// `OAuthLoginCallbacks.onManualCodeInput`.
pub type OnManualCodeInputFunction = Rc<dyn Fn() -> LocalBoxFuture<'static, PiResult<String>>>;
/// `OAuthLoginCallbacks.onSelect`.
pub type OnSelectFunction = Rc<dyn Fn(OAuthSelectPrompt) -> LocalBoxFuture<'static, PiResult<Option<String>>>>;

/// Callback surface retained only for coding-agent extension compatibility.
#[derive(Clone)]
pub struct OAuthLoginCallbacks {
	pub on_auth: OnAuthFunction,
	pub on_device_code: OnDeviceCodeFunction,
	pub on_prompt: OnPromptFunction,
	pub on_progress: Option<OnProgressFunction>,
	pub on_manual_code_input: Option<OnManualCodeInputFunction>,
	pub on_select: OnSelectFunction,
	pub signal: Option<AbortSignal>,
}

//! Port of `packages/ai/src/utils/models-error.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w2-ai-core
// Signatures copied from w2-ai-core's port on `capy/rust-port-w2-ai-core` (47702fe36), which replaces this stub.
use pi_host::PiError;
use serde::{Deserialize, Serialize};

use crate::utils::diagnostics::Thrown;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelsErrorCode {
	#[serde(rename = "model_source")]
	ModelSource,
	#[serde(rename = "model_validation")]
	ModelValidation,
	#[serde(rename = "provider")]
	Provider,
	#[serde(rename = "stream")]
	Stream,
	#[serde(rename = "auth")]
	Auth,
	#[serde(rename = "oauth")]
	Oauth,
}

/// `class ModelsError extends Error`. `ModelsError::new` returns the thrown `PiError`.
pub struct ModelsError;

impl ModelsError {
	/// `new ModelsError(code, message, { cause })`.
	#[allow(clippy::new_ret_no_self, reason = "a thrown ModelsError is a PiError")]
	pub fn new<'a>(_code: ModelsErrorCode, _message: &str, _cause: Option<Thrown<'a>>) -> PiError {
		todo!("PORT-STUB")
	}

	/// `error instanceof ModelsError ? error.code : undefined`.
	pub fn code_of(_error: &PiError) -> Option<ModelsErrorCode> {
		todo!("PORT-STUB")
	}

	/// `error instanceof ModelsError`.
	pub fn is(_error: &PiError) -> bool {
		todo!("PORT-STUB")
	}
}

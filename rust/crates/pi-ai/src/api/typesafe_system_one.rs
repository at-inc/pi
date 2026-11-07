//! Port of `packages/ai/src/api/typesafe-system-one.ts`.
//! PORT-STATUS: ported

use pi_host::{LocalBoxFuture, PiError, PiResult};
use serde_json::{Map, Value};

use super::system_one_shared::{
	ResolvedSystemOneCompat, SystemOneTransport, SystemOneWireRequest, classify_system_one, join_base_url,
};
use crate::providers::compat_schema::SystemOneImageInput;
use crate::types::{ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierResult};

/// TypeSafe's native System One protocol. OpenRouter serves the same protocol,
/// so both providers use this API with different base URLs.
///
/// TypeSafe takes images as `assets`. They are experimental and need the
/// `TypeSafe-Danger-Zone-Images` opt-in header; without it the API answers 400.
/// OpenRouter drops `assets` silently and takes images as `image_url` parts in the state instead.
/// Models behind a proxy that hides both the provider ID and the OpenRouter host must set
/// `compat.imageInput`.
static TRANSPORT: SystemOneTransport = SystemOneTransport {
	api: "typesafe-system-one",
	label: "System One API",
	detect_compat,
	url: |model| join_base_url(&model.base_url, "systemone"),
	payload,
	output: |body| match body {
		Value::Object(body) => Ok(body),
		_ => Err(PiError::new("System One API returned an unexpected response")),
	},
};

/// OpenRouter by provider ID, which survives baseUrl overrides, or by host, which covers regional endpoints.
fn detect_compat(model: &ClassifierModel) -> ResolvedSystemOneCompat {
	if *model.provider == *"openrouter" || model.base_url.contains("openrouter.ai") {
		ResolvedSystemOneCompat {
			image_input: SystemOneImageInput::StateImageUrls,
			send_image_opt_in_header: false,
		}
	} else {
		ResolvedSystemOneCompat {
			image_input: SystemOneImageInput::Assets,
			send_image_opt_in_header: true,
		}
	}
}

/// `{ model: model.id, ...request }`.
fn payload(model: &ClassifierModel, request: SystemOneWireRequest) -> Value {
	let mut payload = Map::new();
	payload.insert("model".into(), Value::String(model.id.clone()));
	payload.extend(request);
	Value::Object(payload)
}

/// TypeSafe System One classification with public `bool` values mapped to wire-level `noul`.
pub fn classify(
	model: &ClassifierModel,
	context: &ClassifierContext,
	options: Option<ClassifierOptions>,
) -> LocalBoxFuture<'static, PiResult<ClassifierResult>> {
	classify_system_one(&TRANSPORT, model, context, options)
}

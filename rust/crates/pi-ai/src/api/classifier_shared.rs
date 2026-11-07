//! Port of `packages/ai/src/api/classifier-shared.ts`.
//! PORT-STATUS: pending

// PORT-STUB: w3-bedrock-misc
// Predicted by w3-openai-responses for `openai_decisions.rs`. A `ClassifierHttpError` is a `PiError` with
// `status` set and `data: { headers, body }`.
use pi_host::PiResult;
use serde_json::Value;

use crate::types::{ClassifierModel, ClassifierOptions, Usage};

pub fn is_record(_value: &Value) -> bool {
	todo!("PORT-STUB")
}

/// `requiredNumber(label, value, field)`; `None` is `undefined`.
pub fn required_number(_label: &str, _value: Option<&Value>, _field: &str) -> PiResult<f64> {
	todo!("PORT-STUB")
}

pub async fn post_classifier_request(
	_label: &str,
	_url: &str,
	_model: &ClassifierModel,
	_body: Value,
	_options: Option<&ClassifierOptions>,
	_no_retry_statuses: Option<&[u16]>,
) -> PiResult<Value> {
	todo!("PORT-STUB")
}

/// `parseClassifierUsage(value, model)`; `None` is `undefined`.
pub fn parse_classifier_usage(_value: Option<&Value>, _model: &ClassifierModel) -> Option<Usage> {
	todo!("PORT-STUB")
}

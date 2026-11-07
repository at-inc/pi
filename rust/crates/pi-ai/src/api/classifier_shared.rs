//! Port of `packages/ai/src/api/classifier-shared.ts`.
//! PORT-STATUS: ported
//!
//! A TS `ClassifierHttpError` is an `Error` with own `status`, `headers` and `body` properties. The Rust port keeps
//! `status` in `PiError.status` and the rest in `PiError.data` as `{ headers, body }`, where `headers` is the
//! response header record (lowercase, sorted names, like `Object.fromEntries(response.headers)`) or `null` for the
//! timeout error, so `retry_provider_request` and `normalize_provider_error` read them like the TS fields. `url` is
//! a parsed `url::Url`; `String(url)` is its serialization.

use bytes::Bytes;
use indexmap::IndexMap;
use pi_host::js::{json_stringify, number_to_string};
use pi_host::{AbortSignal, HttpRequest, PiError, PiResult};
use serde_json::{Value, json};
use url::Url;

use crate::models::calculate_cost;
use crate::types::{ClassifierModel, ClassifierOptions, ProviderHeaders, ProviderResponse, Usage, UsageCost};
use crate::utils::headers::{headers_to_record, provider_headers_to_record};
use crate::utils::identity::{ClientIdentity, ClientIdentityTarget, get_identity_headers};
use crate::utils::provider_retry::{ProviderRetryOptions, retry_provider_request};

/// `httpError(label, response, body)`.
fn http_error(label: &str, status: u16, headers: &IndexMap<String, String>, body: String) -> PiError {
	PiError::new(format!("{label} returned {status}"))
		.with_status(status)
		.with_data(json!({ "headers": headers_to_record(headers), "body": body }))
}

fn timeout_error(timeout_ms: f64) -> PiError {
	PiError::named(
		"TimeoutError",
		format!("Request timed out after {}ms", number_to_string(timeout_ms)),
	)
	.with_data(json!({ "headers": null, "body": "" }))
}

/// `isRecord(value)`: a non-null, non-array object. `None` is TS `undefined`.
pub fn is_record(value: Option<&Value>) -> bool {
	matches!(value, Some(Value::Object(_)))
}

/// `requiredNumber(label, value, field)`. `None` is TS `undefined`.
pub fn required_number(label: &str, value: Option<&Value>, field: &str) -> PiResult<f64> {
	match value.and_then(Value::as_f64) {
		Some(number) => Ok(number),
		None => Err(PiError::new(format!("{label} returned an invalid {field}"))),
	}
}

fn request_headers(
	model: &ClassifierModel,
	api_key: &str,
	options_headers: Option<&ProviderHeaders>,
	identity: Option<&ClientIdentity>,
) -> PiResult<IndexMap<String, String>> {
	let identity_headers: ProviderHeaders = get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		identity,
	)?
	.into_iter()
	.map(|(name, value)| (name, Some(value)))
	.collect();
	let auth_headers = ProviderHeaders::from([
		("authorization".to_string(), Some(format!("Bearer {api_key}"))),
		("content-type".to_string(), Some("application/json".to_string())),
	]);
	let model_headers: Option<ProviderHeaders> = model.headers.as_ref().map(|headers| {
		headers
			.iter()
			.map(|(name, value)| (name.clone(), Some(value.clone())))
			.collect()
	});
	Ok(provider_headers_to_record(&[
		Some(&identity_headers),
		Some(&auth_headers),
		model_headers.as_ref(),
		options_headers,
	])
	.unwrap_or_default())
}

/// Posts one JSON classifier request with bearer auth, `onPayload`/`onResponse` hooks, a fresh
/// timeout per attempt, and provider retries. Returns the parsed response body; throws on failure.
/// `no_retry_statuses` lists HTTP statuses that fail at once although they are normally retried.
pub async fn post_classifier_request(
	label: &str,
	url: &Url,
	model: &ClassifierModel,
	body: Value,
	options: Option<&ClassifierOptions>,
	no_retry_statuses: Option<&[u16]>,
) -> PiResult<Value> {
	let Some((options, api_key)) = options.and_then(|options| {
		let api_key = options.api_key.as_deref().filter(|key| !key.is_empty())?;
		Some((options, api_key))
	}) else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};
	let mut payload = body;
	if let Some(on_payload) = &options.on_payload
		&& let Some(transformed) = on_payload(payload.clone(), model).await?
	{
		payload = transformed;
	}
	let request_body = Bytes::from(json_stringify(&payload));
	let (status, headers, json) = retry_provider_request(
		|| {
			let timeout_signal = options.timeout_ms.map(AbortSignal::timeout);
			let signal = match (&options.signal, &timeout_signal) {
				(Some(signal), Some(timeout_signal)) => {
					Some(AbortSignal::any(&[signal.clone(), timeout_signal.clone()]))
				}
				(signal, timeout_signal) => signal.clone().or_else(|| timeout_signal.clone()),
			};
			let request_body = request_body.clone();
			async move {
				let attempt = async {
					let request = HttpRequest {
						method: "POST".into(),
						url: url.to_string(),
						headers: request_headers(model, api_key, options.headers.as_ref(), options.identity.as_ref())?,
						body: Some(request_body),
						signal,
					};
					let next = pi_host::fetch(request, options.fetch.as_ref()).await?;
					let (status, headers) = (next.status, next.headers.clone());
					if !next.ok() {
						let text = next.text().await?;
						return Err(http_error(label, status, &headers, text));
					}
					Ok((status, headers, next.json().await?))
				};
				attempt.await.map_err(|error| {
					if timeout_signal.as_ref().is_some_and(AbortSignal::aborted)
						&& !options.signal.as_ref().is_some_and(AbortSignal::aborted)
					{
						return timeout_error(options.timeout_ms.expect("a timeout signal implies timeoutMs"));
					}
					error
				})
			}
		},
		ProviderRetryOptions {
			max_retries: Some(options.max_retries.unwrap_or(2.0)),
			max_retry_delay_ms: options.max_retry_delay_ms,
			signal: options.signal.clone(),
			no_retry_statuses: no_retry_statuses.map(<[u16]>::to_vec),
		},
	)
	.await?;
	if let Some(on_response) = &options.on_response {
		on_response(
			ProviderResponse {
				status,
				headers: headers_to_record(&headers),
			},
			model,
		)
		.await?;
	}
	Ok(json)
}

fn token_count(value: Option<&Value>) -> f64 {
	match value.and_then(Value::as_f64) {
		Some(count) if count > 0.0 => count,
		_ => 0.0,
	}
}

/// Usage from a `{ input_tokens, output_tokens }` object, priced from the model catalog like chat
/// usage. A missing or malformed usage object leaves the result without usage instead of failing it.
pub fn parse_classifier_usage(value: Option<&Value>, model: &ClassifierModel) -> Option<Usage> {
	let Some(Value::Object(value)) = value else {
		return None;
	};
	if !value.contains_key("input_tokens") && !value.contains_key("output_tokens") {
		return None;
	}
	let input = token_count(value.get("input_tokens"));
	let output = token_count(value.get("output_tokens"));
	let mut usage = Usage {
		input,
		output,
		cache_read: 0.0,
		cache_write: 0.0,
		total_tokens: input + output,
		cost: UsageCost::default(),
		..Usage::default()
	};
	calculate_cost(model, &mut usage);
	Some(usage)
}

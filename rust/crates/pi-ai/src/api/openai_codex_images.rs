//! Port of `packages/ai/src/api/openai-codex-images.ts`.
//! PORT-STATUS: reviewed
//!
//! `error instanceof OpenAI.APIError && error.headers && error.status !== undefined` is a `PiError` with a status and an
//! object `data.headers`, the shape `openai_client.rs` gives HTTP status errors. `APIConnectionTimeoutError` is the error
//! `openai_client.rs` throws with the message `Request timed out.` and no status.
//! - A payload hook that turns on `stream` gets the SDK's `Stream` object, which has no `data`: it reads as `{}`.

use base64::Engine;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use indexmap::IndexMap;
use pi_host::js::utf16_len;
use pi_host::{AbortSignal, LocalBoxFuture, PiError, PiResult};
use serde_json::{Value, json};

use crate::api::openai_client::{ApiData, ClientOptions, OpenAI, RequestOptions};
use crate::api::openai_responses_shared::{js_string, truthy};
use crate::types::{
	AssistantImages, ImageContent, ImageModel, ImagesContext, ImagesOptions, ImagesStopReason, ProviderHeaders,
	ProviderResponse, UserContent,
};
use crate::utils::abort_signals::combine_abort_signals;
use crate::utils::error_body::{format_provider_error, normalize_provider_error};
use crate::utils::headers::{headers_to_record, provider_headers_to_record};
use crate::utils::identity::{ClientIdentityTarget, get_identity_headers};
use crate::utils::json_parse::json_parse;
use crate::utils::text::js_trim;
use crate::utils::uuid::uuidv7;

const DEFAULT_IMAGE_TIMEOUT_MS: f64 = 150_000.0;
const MAX_IMAGE_BASE64_CHARS: usize = 24 * 1024 * 1024;

/// `atob(value)` as a byte string: the HTML forgiving-base64 decode, or `None` where `atob` throws.
pub(crate) fn atob(value: &str) -> Option<String> {
	let mut data: String = value
		.chars()
		.filter(|ch| !matches!(ch, '\t' | '\n' | '\u{c}' | '\r' | ' '))
		.collect();
	if data.len().is_multiple_of(4) {
		if data.ends_with("==") {
			data.truncate(data.len() - 2);
		} else if data.ends_with('=') {
			data.truncate(data.len() - 1);
		}
	}
	if data.len() % 4 == 1
		|| !data
			.chars()
			.all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '/')
	{
		return None;
	}
	let engine = GeneralPurpose::new(
		&base64::alphabet::STANDARD,
		GeneralPurposeConfig::new()
			.with_decode_allow_trailing_bits(true)
			.with_decode_padding_mode(DecodePaddingMode::RequireNone),
	);
	let bytes = engine.decode(data).ok()?;
	Some(bytes.into_iter().map(char::from).collect())
}

/// `/^[A-Za-z0-9+/]*={0,2}$/.test(value)`.
fn is_base64_text(value: &str) -> bool {
	let data = value.trim_end_matches('=');
	value.len() - data.len() <= 2
		&& data
			.chars()
			.all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '/')
}

/// Generate or edit images through the native Codex Images API with ChatGPT OAuth.
pub fn generate_images(
	model: &ImageModel,
	context: &ImagesContext,
	options: Option<ImagesOptions>,
) -> LocalBoxFuture<'static, PiResult<AssistantImages>> {
	let mut output = AssistantImages {
		api: model.api.clone(),
		provider: model.provider.clone(),
		model: model.id.clone(),
		output: Vec::new(),
		response_id: None,
		usage: None,
		stop_reason: ImagesStopReason::Stop,
		error_message: None,
		timestamp: pi_host::now_ms(),
	};
	let (model, context) = (model.clone(), context.clone());
	Box::pin(async move {
		let options = options.as_ref();
		let timeout_ms = options
			.and_then(|options| options.timeout_ms)
			.unwrap_or(DEFAULT_IMAGE_TIMEOUT_MS);
		let timeout_signal = if timeout_ms > 0.0 {
			Some(AbortSignal::try_timeout(timeout_ms)?)
		} else {
			None
		};
		let request_signal = combine_abort_signals(&[
			options.and_then(|options| options.signal.as_ref()),
			timeout_signal.as_ref(),
		]);
		let result = request(
			&model,
			&context,
			options,
			timeout_ms,
			request_signal.signal.clone(),
			&mut output,
		)
		.await;
		if let Err(error) = result {
			if let Some(status) = error.status
				&& let Some(Value::Object(headers)) = error.data.as_ref().and_then(|data| data.get("headers"))
			{
				let headers: IndexMap<String, String> = headers
					.iter()
					.map(|(name, value)| (name.clone(), js_string(Some(value))))
					.collect();
				if let Some(on_response) = options.and_then(|options| options.on_response.as_ref()) {
					let response = ProviderResponse {
						status,
						headers: headers_to_record(&headers),
					};
					if let Err(hook_error) = on_response(response, &model).await {
						request_signal.cleanup();
						return Err(hook_error);
					}
				}
				output.response_id = headers_to_record(&headers).get("x-codex-imagegen-request-id").cloned();
			}
			let caller_aborted = options
				.and_then(|options| options.signal.as_ref())
				.is_some_and(AbortSignal::aborted);
			let timed_out = timeout_ms > 0.0
				&& !caller_aborted
				&& (timeout_signal.as_ref().is_some_and(AbortSignal::aborted)
					|| (error.message == "Request timed out." && error.status.is_none()));
			output.stop_reason = if caller_aborted {
				ImagesStopReason::Aborted
			} else {
				ImagesStopReason::Error
			};
			output.error_message = Some(if timed_out {
				format!(
					"ChatGPT image generation timed out after {}ms",
					pi_host::js::number_to_string(timeout_ms)
				)
			} else {
				format_provider_error(&normalize_provider_error(&error), None)
			});
		}
		request_signal.cleanup();
		Ok(output)
	})
}

async fn request(
	model: &ImageModel,
	context: &ImagesContext,
	options: Option<&ImagesOptions>,
	timeout_ms: f64,
	signal: Option<AbortSignal>,
	output: &mut AssistantImages,
) -> PiResult<()> {
	let Some(api_key) = options
		.and_then(|options| options.api_key.as_deref())
		.filter(|api_key| !api_key.is_empty())
	else {
		return Err(PiError::new(format!("No API key for provider: {}", model.provider)));
	};
	let account_id = atob(api_key.split('.').nth(1).unwrap_or("undefined"))
		.and_then(|payload| json_parse(&payload).ok())
		.and_then(|payload| {
			payload
				.get("https://api.openai.com/auth")
				.and_then(|auth| auth.get("chatgpt_account_id"))
				.cloned()
		});
	if !truthy(account_id.as_ref()) {
		return Err(PiError::new("Failed to extract accountId from token"));
	}
	let account_id = js_string(account_id.as_ref());

	let trimmed_base_url = js_trim(&model.base_url);
	let base_url = if trimmed_base_url.is_empty() {
		"https://chatgpt.com/backend-api"
	} else {
		trimmed_base_url
	}
	.trim_end_matches('/');
	let base_url = match base_url.strip_suffix("/codex/responses") {
		Some(prefix) => format!("{prefix}/codex"),
		None => base_url.to_string(),
	};
	let defaults: ProviderHeaders = IndexMap::from([
		("originator".to_string(), Some("pi".to_string())),
		("chatgpt-account-id".to_string(), Some(account_id)),
		("x-codex-image-turn-id".to_string(), Some(uuidv7(None)?)),
	]);
	let identity_headers: ProviderHeaders = get_identity_headers(
		ClientIdentityTarget {
			provider: &model.provider,
			base_url: &model.base_url,
		},
		options.and_then(|options| options.identity.as_ref()),
	)?
	.into_iter()
	.map(|(name, value)| (name, Some(value)))
	.collect();
	let model_headers: Option<ProviderHeaders> = model.headers.as_ref().map(|headers| {
		headers
			.iter()
			.map(|(name, value)| (name.clone(), Some(value.clone())))
			.collect()
	});
	let default_headers = provider_headers_to_record(&[
		Some(&defaults),
		Some(&identity_headers),
		model_headers.as_ref(),
		options.and_then(|options| options.headers.as_ref()),
	])
	.map(|headers| headers.into_iter().map(|(name, value)| (name, Some(value))).collect());
	let client = OpenAI::new(ClientOptions {
		api_key: Some(api_key.to_string()),
		base_url: Some(if base_url.ends_with("/codex") {
			base_url
		} else {
			format!("{base_url}/codex")
		}),
		fetch: options.and_then(|options| options.fetch.clone()),
		default_headers,
		..ClientOptions::default()
	})?;
	let images: Vec<Value> = context
		.input
		.iter()
		.filter_map(|part| match part {
			UserContent::Image(image) => {
				Some(json!({ "image_url": format!("data:{};base64,{}", image.mime_type, image.data) }))
			}
			UserContent::Text(_) => None,
		})
		.collect();
	// Codex rejects the hosted image_generation tool choice and can return only text without it.
	// Use the native contract in openai/codex's codex-rs/codex-api/src/endpoint/images.rs instead.
	let prompt = context
		.input
		.iter()
		.filter_map(|part| match part {
			UserContent::Text(text) => Some(text.text.as_str()),
			UserContent::Image(_) => None,
		})
		.collect::<Vec<_>>()
		.join("\n");
	let has_images = !images.is_empty();
	let mut body = json!({ "model": "gpt-image-2", "prompt": prompt, "n": 1 });
	if has_images {
		body["images"] = Value::Array(images);
	}
	if let Some(on_payload) = options.and_then(|options| options.on_payload.as_ref())
		&& let Some(next_body) = on_payload(body.clone(), model).await?
	{
		if !next_body.is_object() {
			return Err(PiError::new(
				"Codex image generation payload hook must return an object",
			));
		}
		body = next_body;
	}
	let with_response = client
		.post_json(
			if has_images {
				"/images/edits"
			} else {
				"/images/generations"
			},
			&body,
			&RequestOptions {
				signal,
				timeout: (timeout_ms > 0.0).then_some(timeout_ms),
				// Generations are billable: never retry an ambiguous failure.
				max_retries: Some(0.0),
				headers: None,
			},
		)
		.await?;
	let response = with_response.response;
	let data = match with_response.data {
		ApiData::Json(data) => data,
		ApiData::Stream(_) => json!({}),
	};
	if let Some(on_response) = options.and_then(|options| options.on_response.as_ref()) {
		on_response(
			ProviderResponse {
				status: response.status,
				headers: headers_to_record(&response.headers),
			},
			model,
		)
		.await?;
	}
	output.response_id = headers_to_record(&response.headers)
		.get("x-codex-imagegen-request-id")
		.cloned();
	let Some(Value::Array(images)) = data.get("data").filter(|_| data.is_object()) else {
		return Err(PiError::new("ChatGPT image generation completed without an image"));
	};
	if images.is_empty() {
		return Err(PiError::new("ChatGPT image generation completed without an image"));
	}
	let mut total_chars = 0;
	let mut converted = Vec::with_capacity(images.len());
	for image in images {
		let Some(value) = image
			.get("b64_json")
			.and_then(Value::as_str)
			.filter(|value| !value.is_empty() && utf16_len(value).is_multiple_of(4) && is_base64_text(value))
		else {
			return Err(PiError::new("ChatGPT returned malformed image data"));
		};
		total_chars += utf16_len(value);
		if total_chars > MAX_IMAGE_BASE64_CHARS {
			return Err(PiError::new(
				"ChatGPT image generation output exceeds the 24 MiB base64 limit",
			));
		}
		converted.push(UserContent::Image(ImageContent {
			data: value.to_string(),
			mime_type: "image/png".into(),
			..ImageContent::default()
		}));
	}
	output.output = converted;
	Ok(())
}

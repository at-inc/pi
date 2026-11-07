//! Port of `packages/ai/test/google-thinking-level-map.test.ts`.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::api::google_shared::{ResolvedGoogleThinkingLevel, resolve_google_thinking_level};
use pi_ai::api::{google_generative_ai, google_vertex};
use pi_ai::types::{
	BaseModel, Context, Message, Model, ModelCost, ModelInputModality, ModelThinkingLevel, OnPayload,
	ProviderRequestOptions, SimpleStreamOptions, StreamOptions, ThinkingBudgets, ThinkingLevel, ThinkingLevelMap,
	TranscriptContext, UserMessage, UserMessageContent,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn context() -> TranscriptContext {
	normalize_context(&Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("Hello".into()),
			timestamp: 0.0,
			..UserMessage::default()
		})],
		..Context::default()
	})
}

fn thinking_level_map(map: Value) -> ThinkingLevelMap {
	serde_json::from_value(map).expect("valid thinking level map")
}

fn model(id: &str, api: &str, provider: &str, base_url: &str, map: Value) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: api.into(),
			provider: provider.into(),
			base_url: base_url.into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning: true,
		thinking_level_map: Some(thinking_level_map(map)),
		context_window: 128000.0,
		max_tokens: 4096.0,
		..Model::default()
	}
}

fn google_model(id: &str, map: Value) -> Model {
	model(
		id,
		"google-generative-ai",
		"test-google",
		"https://example.invalid/v1beta",
		map,
	)
}

fn vertex_model(id: &str, map: Value) -> Model {
	model(id, "google-vertex", "test-vertex", "https://example.invalid/v1", map)
}

#[derive(Clone, Copy, Debug)]
enum Adapter {
	GoogleGenerativeAI,
	GoogleVertex,
}

const GOOGLE_ADAPTERS: [Adapter; 2] = [Adapter::GoogleGenerativeAI, Adapter::GoogleVertex];

/// `captureGooglePayload` / `captureVertexPayload`: the payload `onPayload` sees; `onPayload` then throws.
async fn capture_payload(
	adapter: Adapter,
	model: Model,
	reasoning: Option<ThinkingLevel>,
	thinking_budgets: Option<ThinkingBudgets>,
) -> Value {
	let payload: Rc<RefCell<Option<Value>>> = Rc::default();
	let captured = payload.clone();
	let on_payload: OnPayload = Rc::new(move |request, _model| {
		*captured.borrow_mut() = Some(request);
		Box::pin(async { Err(PiError::new("payload captured")) })
	});
	let options = SimpleStreamOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("test".into()),
				on_payload: Some(on_payload),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		reasoning,
		thinking_budgets,
		..SimpleStreamOptions::default()
	};
	let stream = match adapter {
		Adapter::GoogleGenerativeAI => google_generative_ai::stream_simple(&model, &context(), Some(options)),
		Adapter::GoogleVertex => google_vertex::stream_simple(&model, &context(), Some(options)),
	}
	.expect("stream starts");
	let result = stream.result().await.expect("stream result");

	assert!(
		result
			.error_message
			.as_deref()
			.unwrap_or_default()
			.contains("payload captured"),
		"{:?}",
		result.error_message
	);
	payload
		.take()
		.unwrap_or_else(|| panic!("{adapter:?} payload was not captured"))
}

fn thinking_config(payload: &Value) -> &Value {
	&payload["config"]["thinkingConfig"]
}

mod google_thinking_level_maps {
	use super::*;

	#[test]
	fn exhaustively_resolves_supported_logical_levels_and_mapping_values() {
		let default_expectations = [
			(ModelThinkingLevel::Minimal, ResolvedGoogleThinkingLevel::Minimal),
			(ModelThinkingLevel::Low, ResolvedGoogleThinkingLevel::Low),
			(ModelThinkingLevel::Medium, ResolvedGoogleThinkingLevel::Medium),
			(ModelThinkingLevel::High, ResolvedGoogleThinkingLevel::High),
		];
		for (level, expected) in default_expectations {
			assert_eq!(
				resolve_google_thinking_level(&google_model("gemini-3.7-flash", json!({})), level).unwrap(),
				expected
			);
		}

		let mapped_expectations = [
			("minimal", ResolvedGoogleThinkingLevel::Minimal),
			("low", ResolvedGoogleThinkingLevel::Low),
			("medium", ResolvedGoogleThinkingLevel::Medium),
			("high", ResolvedGoogleThinkingLevel::High),
			("MINIMAL", ResolvedGoogleThinkingLevel::Minimal),
			("LOW", ResolvedGoogleThinkingLevel::Low),
			("MEDIUM", ResolvedGoogleThinkingLevel::Medium),
			("HIGH", ResolvedGoogleThinkingLevel::High),
		];
		for (mapped, expected) in mapped_expectations {
			let model = google_model(
				"gemini-3.7-flash",
				json!({ "high": mapped, "xhigh": mapped, "max": mapped }),
			);
			assert_eq!(
				resolve_google_thinking_level(&model, ModelThinkingLevel::High).unwrap(),
				expected
			);
			assert_eq!(
				resolve_google_thinking_level(&model, ModelThinkingLevel::Xhigh).unwrap(),
				expected
			);
			assert_eq!(
				resolve_google_thinking_level(&model, ModelThinkingLevel::Max).unwrap(),
				expected
			);
		}

		let invalid_model = google_model("gemini-3.7-flash", json!({ "xhigh": "extreme" }));
		assert!(
			resolve_google_thinking_level(&invalid_model, ModelThinkingLevel::Xhigh)
				.unwrap_err()
				.message
				.contains(
					"Unsupported Google thinking level mapping for test-google/gemini-3.7-flash: xhigh -> extreme"
				)
		);
		assert!(
			resolve_google_thinking_level(&google_model("gemini-3.7-flash", json!({})), ModelThinkingLevel::Max)
				.unwrap_err()
				.message
				.contains(
					"Unsupported Google thinking level mapping for test-google/gemini-3.7-flash: max -> undefined"
				)
		);
	}

	fn adapter_model(adapter: Adapter, id: &str, map: Value) -> Model {
		match adapter {
			Adapter::GoogleGenerativeAI => google_model(id, map),
			Adapter::GoogleVertex => vertex_model(id, map),
		}
	}

	// Regression test for https://github.com/earendil-works/pi/issues/9455
	#[test]
	fn uses_the_lowest_supported_level_when_reasoning_is_omitted() {
		for adapter in GOOGLE_ADAPTERS {
			run(|_host| async move {
				let payload = capture_payload(
					adapter,
					adapter_model(
						adapter,
						"gemini-3.8-flash",
						json!({
							"off": null, "minimal": null, "low": "low", "medium": "medium", "high": "high",
							"xhigh": null, "max": null,
						}),
					),
					None,
					None,
				)
				.await;

				assert_eq!(
					thinking_config(&payload),
					&json!({ "thinkingLevel": "LOW" }),
					"{adapter:?}"
				);
			});
		}
	}

	#[test]
	fn preserves_native_medium_effort_for_gemini_3_1_pro() {
		for adapter in GOOGLE_ADAPTERS {
			run(|_host| async move {
				let payload = capture_payload(
					adapter,
					adapter_model(
						adapter,
						"gemini-3.1-pro-preview",
						json!({
							"off": null, "minimal": null, "low": "low", "medium": "medium", "high": "high",
							"xhigh": null, "max": null,
						}),
					),
					Some(ThinkingLevel::Medium),
					None,
				)
				.await;

				assert_eq!(
					thinking_config(&payload),
					&json!({ "includeThoughts": true, "thinkingLevel": "MEDIUM" }),
					"{adapter:?}"
				);
			});
		}
	}

	#[test]
	fn disables_gemini_2_5_thinking_when_reasoning_is_omitted() {
		for adapter in GOOGLE_ADAPTERS {
			run(|_host| async move {
				let payload = capture_payload(
					adapter,
					adapter_model(adapter, "gemini-2.5-flash", json!({})),
					None,
					None,
				)
				.await;

				assert_eq!(
					thinking_config(&payload),
					&json!({ "thinkingBudget": 0.0 }),
					"{adapter:?}"
				);
			});
		}
	}

	// Regression test for https://github.com/earendil-works/pi/issues/10890
	#[test]
	fn uses_thinking_levels_for_gemini_4_models() {
		for adapter in GOOGLE_ADAPTERS {
			run(|_host| async move {
				let payload = capture_payload(
					adapter,
					adapter_model(adapter, "gemini-4-argon", json!({})),
					Some(ThinkingLevel::Medium),
					None,
				)
				.await;

				assert_eq!(
					thinking_config(&payload),
					&json!({ "includeThoughts": true, "thinkingLevel": "MEDIUM" }),
					"{adapter:?}"
				);
			});
		}
	}

	#[test]
	fn maps_google_generative_ai_extended_levels_to_a_supported_level() {
		for reasoning in [ThinkingLevel::Xhigh, ThinkingLevel::Max] {
			run(|_host| async move {
				let payload = capture_payload(
					Adapter::GoogleGenerativeAI,
					google_model("gemini-3.7-flash", json!({ "xhigh": "high", "max": "high" })),
					Some(reasoning),
					None,
				)
				.await;

				assert_eq!(
					thinking_config(&payload),
					&json!({ "includeThoughts": true, "thinkingLevel": "HIGH" }),
					"{reasoning:?}"
				);
			});
		}
	}

	#[test]
	fn honors_uppercase_provider_values_for_standard_google_generative_ai_levels() {
		run(|_host| async move {
			let payload = capture_payload(
				Adapter::GoogleGenerativeAI,
				google_model("gemini-3.7-flash", json!({ "high": "LOW" })),
				Some(ThinkingLevel::High),
				None,
			)
			.await;

			assert_eq!(thinking_config(&payload)["thinkingLevel"], json!("LOW"));
		});
	}

	#[test]
	fn uses_mapped_google_generative_ai_levels_for_token_budgets() {
		run(|_host| async move {
			let payload = capture_payload(
				Adapter::GoogleGenerativeAI,
				google_model("gemini-2.5-flash", json!({ "xhigh": "high" })),
				Some(ThinkingLevel::Xhigh),
				Some(ThinkingBudgets {
					high: Some(1234.0),
					..ThinkingBudgets::default()
				}),
			)
			.await;

			assert_eq!(thinking_config(&payload)["thinkingBudget"], json!(1234.0));
		});
	}

	#[test]
	fn maps_google_vertex_extended_levels() {
		run(|_host| async move {
			let payload = capture_payload(
				Adapter::GoogleVertex,
				vertex_model("gemini-3.7-flash", json!({ "xhigh": "high" })),
				Some(ThinkingLevel::Xhigh),
				None,
			)
			.await;

			assert_eq!(
				thinking_config(&payload),
				&json!({ "includeThoughts": true, "thinkingLevel": "HIGH" })
			);
		});
	}

	#[test]
	fn uses_mapped_google_vertex_levels_for_token_budgets() {
		run(|_host| async move {
			let payload = capture_payload(
				Adapter::GoogleVertex,
				vertex_model("gemini-2.5-flash", json!({ "max": "high" })),
				Some(ThinkingLevel::Max),
				Some(ThinkingBudgets {
					high: Some(4321.0),
					..ThinkingBudgets::default()
				}),
			)
			.await;

			assert_eq!(thinking_config(&payload)["thinkingBudget"], json!(4321.0));
		});
	}
}

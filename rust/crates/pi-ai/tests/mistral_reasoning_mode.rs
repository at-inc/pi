//! Port of `packages/ai/test/mistral-reasoning-mode.test.ts`.
//!
//! The model's base URL is `http://127.0.0.1:9`, so the request fails after `onPayload` captured the payload; the
//! test host's fetch fails the same way.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::api::mistral_conversations::stream_simple;
use pi_ai::types::{
	BaseModel, CacheRetention, Context, Message, Model, ModelCost, ModelInputModality, OnPayload,
	ProviderRequestOptions, SimpleStreamOptions, StreamOptions, ThinkingLevel, ThinkingLevelMap, UserMessage,
	UserMessageContent,
};
use pi_ai::utils::transcript::normalize_context;
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

fn none_high_levels() -> Value {
	json!({ "off": "none", "minimal": null, "low": null, "medium": null, "high": "high", "xhigh": null, "max": null })
}

fn glm_5_2_levels() -> Value {
	let mut levels = none_high_levels();
	levels["max"] = json!("max");
	levels
}

fn glm_5_3_levels() -> Value {
	let mut levels = none_high_levels();
	levels["off"] = Value::Null;
	levels["low"] = json!("low");
	levels["max"] = json!("max");
	levels
}

fn make_model(id: &str, reasoning: bool, thinking_level_map: Option<Value>) -> Model {
	Model {
		base: BaseModel {
			id: id.into(),
			name: id.into(),
			api: "mistral-conversations".into(),
			provider: "mistral".into(),
			base_url: "http://127.0.0.1:9".into(),
			input: vec![ModelInputModality::Text],
			cost: ModelCost::default(),
			..BaseModel::default()
		},
		reasoning,
		thinking_level_map: thinking_level_map
			.map(|map| serde_json::from_value::<ThinkingLevelMap>(map).expect("valid thinking level map")),
		context_window: 128000.0,
		max_tokens: 16384.0,
		..Model::default()
	}
}

fn make_context() -> Context {
	Context {
		messages: vec![Message::User(UserMessage {
			content: UserMessageContent::Text("Hello".into()),
			timestamp: 0.0,
			..UserMessage::default()
		})],
		..Context::default()
	}
}

#[derive(Default)]
struct Overrides {
	reasoning: Option<ThinkingLevel>,
	session_id: Option<&'static str>,
	cache_retention: Option<CacheRetention>,
}

/// `capturePayload(model, options)`.
async fn capture_payload(model: Model, overrides: Overrides) -> Value {
	let payload: Rc<RefCell<Option<Value>>> = Rc::default();
	let captured = payload.clone();
	let on_payload: OnPayload = Rc::new(move |request, _model| {
		*captured.borrow_mut() = Some(request.clone());
		Box::pin(async move { Ok(Some(request)) })
	});
	let options = SimpleStreamOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				api_key: Some("fake-key".into()),
				on_payload: Some(on_payload),
				..ProviderRequestOptions::default()
			},
			session_id: overrides.session_id.map(String::from),
			cache_retention: overrides.cache_retention,
			..StreamOptions::default()
		},
		reasoning: overrides.reasoning,
		..SimpleStreamOptions::default()
	};
	stream_simple(&model, &normalize_context(&make_context()), Some(options))
		.expect("stream starts")
		.result()
		.await
		.expect("stream result");

	payload
		.take()
		.expect("Expected payload to be captured before request failure")
}

fn reasoning(level: ThinkingLevel) -> Overrides {
	Overrides {
		reasoning: Some(level),
		..Overrides::default()
	}
}

fn with_failing_fetch(host: &pi_host::testing::TestHost) {
	host.on_fetch(|_| Box::pin(async { Err(PiError::new("connect ECONNREFUSED 127.0.0.1:9")) }));
}

mod mistral_reasoning_mode_selection {
	use super::*;

	#[test]
	fn uses_prompt_mode_for_reasoning_models_without_a_thinking_level_map_magistral() {
		run(|host| async move {
			with_failing_fetch(&host);
			let payload = capture_payload(
				make_model("magistral-medium-latest", true, None),
				reasoning(ThinkingLevel::Medium),
			)
			.await;

			assert_eq!(payload.get("promptMode"), Some(&json!("reasoning")));
			assert_eq!(payload.get("reasoningEffort"), None);
		});
	}

	#[test]
	fn omits_reasoning_controls_for_magistral_when_thinking_is_off() {
		run(|host| async move {
			with_failing_fetch(&host);
			let payload =
				capture_payload(make_model("magistral-medium-latest", true, None), Overrides::default()).await;

			assert_eq!(payload.get("promptMode"), None);
			assert_eq!(payload.get("reasoningEffort"), None);
		});
	}

	// Regression for #8700 and #9375: Medium and GLM-5.2 ignore Magistral's prompt_mode.
	const EFFORT_MODEL_IDS: [&str; 3] = ["mistral-small-2603", "mistral-medium-latest", "zai-glm-5-2"];

	fn effort_map(model_id: &str) -> Value {
		if model_id == "zai-glm-5-2" {
			glm_5_2_levels()
		} else {
			none_high_levels()
		}
	}

	#[test]
	fn uses_reasoning_effort_when_thinking_is_enabled() {
		for model_id in EFFORT_MODEL_IDS {
			run(move |host| async move {
				with_failing_fetch(&host);
				let payload = capture_payload(
					make_model(model_id, true, Some(effort_map(model_id))),
					reasoning(ThinkingLevel::High),
				)
				.await;

				assert_eq!(payload.get("reasoningEffort"), Some(&json!("high")), "{model_id}");
				assert_eq!(payload.get("promptMode"), None, "{model_id}");
			});
		}
	}

	#[test]
	fn clamps_unsupported_levels_to_a_supported_effort() {
		for model_id in EFFORT_MODEL_IDS {
			run(move |host| async move {
				with_failing_fetch(&host);
				let payload = capture_payload(
					make_model(model_id, true, Some(effort_map(model_id))),
					reasoning(ThinkingLevel::Low),
				)
				.await;

				assert_eq!(payload.get("reasoningEffort"), Some(&json!("high")), "{model_id}");
			});
		}
	}

	#[test]
	fn sends_reasoning_effort_none_when_thinking_is_off() {
		for model_id in EFFORT_MODEL_IDS {
			run(move |host| async move {
				with_failing_fetch(&host);
				let payload = capture_payload(
					make_model(model_id, true, Some(effort_map(model_id))),
					Overrides::default(),
				)
				.await;

				assert_eq!(payload.get("reasoningEffort"), Some(&json!("none")), "{model_id}");
				assert_eq!(payload.get("promptMode"), None, "{model_id}");
			});
		}
	}

	// Regression for #9678: requested levels must reach Mistral-hosted GLM models.
	#[test]
	fn sends_max_for_glm_5_2() {
		run(|host| async move {
			with_failing_fetch(&host);
			let payload = capture_payload(
				make_model("zai-glm-5-2", true, Some(glm_5_2_levels())),
				reasoning(ThinkingLevel::Max),
			)
			.await;

			assert_eq!(payload.get("reasoningEffort"), Some(&json!("max")));
		});
	}

	mod zai_glm_5_3 {
		use super::*;

		#[test]
		fn sends_reasoning_effort_for_each_supported_level() {
			for (level, effort) in [
				(ThinkingLevel::Low, "low"),
				(ThinkingLevel::High, "high"),
				(ThinkingLevel::Max, "max"),
			] {
				run(move |host| async move {
					with_failing_fetch(&host);
					let payload = capture_payload(
						make_model("zai-glm-5-3", true, Some(glm_5_3_levels())),
						reasoning(level),
					)
					.await;

					assert_eq!(payload.get("reasoningEffort"), Some(&json!(effort)));
					assert_eq!(payload.get("promptMode"), None);
				});
			}
		}

		#[test]
		fn maps_medium_to_high() {
			run(|host| async move {
				with_failing_fetch(&host);
				let payload = capture_payload(
					make_model("zai-glm-5-3", true, Some(glm_5_3_levels())),
					reasoning(ThinkingLevel::Medium),
				)
				.await;

				assert_eq!(payload.get("reasoningEffort"), Some(&json!("high")));
			});
		}
	}

	// Regression for #8700: reasoning controls must respect the model's reasoning capability.
	#[test]
	fn omits_reasoning_controls_for_non_reasoning_models() {
		run(|host| async move {
			with_failing_fetch(&host);
			let payload = capture_payload(
				make_model("mistral-medium-2505", false, None),
				reasoning(ThinkingLevel::Medium),
			)
			.await;

			assert_eq!(payload.get("reasoningEffort"), None);
			assert_eq!(payload.get("promptMode"), None);
		});
	}

	#[test]
	fn uses_the_session_id_as_prompt_cache_key() {
		run(|host| async move {
			with_failing_fetch(&host);
			let payload = capture_payload(
				make_model("mistral-large-latest", false, None),
				Overrides {
					session_id: Some("session-123"),
					..Overrides::default()
				},
			)
			.await;

			assert_eq!(payload.get("promptCacheKey"), Some(&json!("session-123")));
		});
	}

	#[test]
	fn omits_prompt_cache_key_when_cache_retention_is_disabled() {
		run(|host| async move {
			with_failing_fetch(&host);
			let payload = capture_payload(
				make_model("mistral-large-latest", false, None),
				Overrides {
					session_id: Some("session-123"),
					cache_retention: Some(CacheRetention::None),
					..Overrides::default()
				},
			)
			.await;

			assert_eq!(payload.get("promptCacheKey"), None);
		});
	}
}

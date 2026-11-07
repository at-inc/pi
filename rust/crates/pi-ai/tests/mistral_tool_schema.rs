//! Port of `packages/ai/test/mistral-tool-schema.test.ts`.
//!
//! TypeBox's symbol keys have no Rust counterpart: a `serde_json::Value` schema holds only string keys, so the
//! check that the payload carries no symbol keys is the check that the parameters reach the payload as plain JSON.
//! The request goes to `http://127.0.0.1:9`; the test host's fetch fails it like the refused connection.

use std::cell::RefCell;
use std::rc::Rc;

use pi_ai::compat::complete;
use pi_ai::providers::all::get_builtin_model;
use pi_ai::types::{
	ConstrainedSamplingConfig, ConstrainedSamplingStrictness, Context, Message, OnPayload, ProviderRequestOptions,
	ProviderStreamOptions, StopReason, StreamOptions, Tool, ToolConstrainedSampling, UserMessage, UserMessageContent,
};
use pi_host::PiError;
use pi_host::testing::run;
use serde_json::{Value, json};

mod mistral_tool_schema_serialization {
	use super::*;

	#[test]
	fn strips_typebox_symbol_keys_before_the_sdk_validates_tool_schemas() {
		run(|host| async move {
			host.on_fetch(|_| Box::pin(async { Err(PiError::new("connect ECONNREFUSED 127.0.0.1:9")) }));
			let mut model = get_builtin_model("mistral", "devstral-medium-latest").expect("built-in model");
			model.base_url = "http://127.0.0.1:9".into();
			let parameters = json!({
				"type": "object",
				"properties": {
					"nested": {
						"type": "object",
						"properties": { "value": { "type": "string" } },
						"required": ["value"],
					},
				},
				"required": ["nested"],
			});
			let context = Context {
				messages: vec![Message::User(UserMessage {
					content: UserMessageContent::Text("Hi".into()),
					timestamp: 0.0,
					..UserMessage::default()
				})],
				tools: Some(vec![Tool {
					name: "inspect_schema".into(),
					description: "Inspect the schema".into(),
					parameters,
					constrained_sampling: Some(ToolConstrainedSampling::Config(
						ConstrainedSamplingConfig::JsonSchema {
							strict: ConstrainedSamplingStrictness::Require,
						},
					)),
					..Tool::default()
				}]),
				..Context::default()
			};
			let captured_payload: Rc<RefCell<Option<Value>>> = Rc::default();
			let captured = captured_payload.clone();
			let on_payload: OnPayload = Rc::new(move |payload, _model| {
				*captured.borrow_mut() = Some(payload.clone());
				Box::pin(async move { Ok(Some(payload)) })
			});
			let options = ProviderStreamOptions {
				base: StreamOptions {
					base: ProviderRequestOptions {
						api_key: Some("fake-key".into()),
						on_payload: Some(on_payload),
						..ProviderRequestOptions::default()
					},
					..StreamOptions::default()
				},
				..ProviderStreamOptions::default()
			};

			let response = complete(&model, &context, Some(options))
				.await
				.expect("complete")
				.expect("assistant message");

			let payload = captured_payload.take().expect("payload captured");
			let tools = payload["tools"].as_array().expect("tools array");
			assert_eq!(tools.len(), 1);
			assert_eq!(tools[0]["function"]["strict"], json!(true));
			let payload_parameters = &tools[0]["function"]["parameters"];
			assert!(payload_parameters.is_object());
			let properties = &payload_parameters["properties"];
			assert!(properties.is_object());
			assert!(properties["nested"].is_object());
			assert_eq!(response.stop_reason, StopReason::Error);
			assert!(
				!response
					.error_message
					.as_deref()
					.unwrap_or_default()
					.contains("Input validation failed")
			);
		});
	}
}

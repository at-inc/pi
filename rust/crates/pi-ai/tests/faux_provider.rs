//! Port of `packages/ai/test/faux-provider.test.ts`.
//!
//! The TS suite drives the faux core through compat's `registerFauxProvider`, `stream` and `complete`
//! (`compat.ts`, still pending in w2-ai-auth). Those three are test doubles here with compat's behavior for a faux
//! model: a registry keyed by api, `normalizeContext`, then the core's `stream`; an unregistered api throws
//! `No API provider registered for api: ...`. Compat's env API key injection finds no key for the faux provider, so
//! the doubles skip it. Each test thread has its own registry and every registration gets a random api, so the
//! `afterEach` unregister loop is not needed.

use std::cell::RefCell;
use std::rc::Rc;

use indexmap::IndexMap;
use pi_ai::providers::faux::{
	FauxAssistantMessageOptions, FauxCore, FauxModelDefinition, FauxProviderRegistration, FauxResponseStep,
	FauxTokenSize, FauxToolCallOptions, RegisterFauxProviderOptions, create_faux_core, faux_assistant_message,
	faux_text, faux_thinking, faux_tool_call,
};
use pi_ai::types::{
	AssistantContent, AssistantMessage, AssistantMessageEvent, CacheRetention, Context, ImageContent, JsonObject,
	Message, Model, ProviderRequestOptions, SimpleStreamOptions, StopReason, StreamOptions, TextContent,
	ThinkingContent, Tool, ToolCall, ToolResultMessage, UserContent, UserMessage, UserMessageContent,
};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_ai::utils::transcript::normalize_context;
use pi_host::js::utf16_len;
use pi_host::{AbortController, PiError, PiResult};
use serde_json::{Value, json};

thread_local! {
	static API_PROVIDERS: RefCell<IndexMap<String, (FauxCore, String)>> = RefCell::new(IndexMap::new());
	static NEXT_SOURCE_ID: RefCell<u64> = const { RefCell::new(0) };
}

/// Compat's `registerFauxProvider(options)`.
fn register_faux_provider(options: Option<RegisterFauxProviderOptions>) -> FauxProviderRegistration {
	let core = create_faux_core(options.unwrap_or_default());
	let source_id = NEXT_SOURCE_ID.with(|next| {
		*next.borrow_mut() += 1;
		format!("faux-provider-{}", next.borrow())
	});
	API_PROVIDERS.with(|providers| {
		providers
			.borrow_mut()
			.insert(core.api.clone(), (core.clone(), source_id.clone()))
	});
	FauxProviderRegistration {
		core,
		unregister: Rc::new(move || {
			API_PROVIDERS.with(|providers| providers.borrow_mut().retain(|_, (_, source)| *source != source_id));
		}),
	}
}

/// Compat's `stream(model, context, options)` for a faux model.
fn stream(
	model: &Model,
	context: &Context,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessageEventStream> {
	let transcript = normalize_context(context);
	let provider = API_PROVIDERS.with(|providers| providers.borrow().get(&*model.api).map(|(core, _)| core.clone()));
	let Some(provider) = provider else {
		return Err(PiError::new(format!(
			"No API provider registered for api: {}",
			model.api
		)));
	};
	Ok(provider.stream(model, &transcript, options))
}

/// Compat's `complete(model, context, options)`.
async fn complete(
	model: &Model,
	context: &Context,
	options: Option<SimpleStreamOptions>,
) -> PiResult<AssistantMessage> {
	let stream = stream(model, context, options)?;
	Ok(stream.result().await.expect("the faux stream ends with a message"))
}

async fn collect_events(stream: AssistantMessageEventStream) -> Vec<AssistantMessageEvent> {
	let mut events = Vec::new();
	while let Some(event) = stream.next().await {
		events.push(event);
	}
	events
}

fn event_type(event: &AssistantMessageEvent) -> &'static str {
	match event {
		AssistantMessageEvent::Start { .. } => "start",
		AssistantMessageEvent::TextStart { .. } => "text_start",
		AssistantMessageEvent::TextDelta { .. } => "text_delta",
		AssistantMessageEvent::TextEnd { .. } => "text_end",
		AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
		AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
		AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
		AssistantMessageEvent::ToolcallStart { .. } => "toolcall_start",
		AssistantMessageEvent::ToolcallDelta { .. } => "toolcall_delta",
		AssistantMessageEvent::ToolcallEnd { .. } => "toolcall_end",
		AssistantMessageEvent::Done { .. } => "done",
		AssistantMessageEvent::Error { .. } => "error",
	}
}

fn user(content: &str, timestamp: f64) -> Message {
	Message::User(UserMessage {
		content: UserMessageContent::Text(content.to_string()),
		timestamp,
		..UserMessage::default()
	})
}

fn hi() -> Context {
	Context {
		messages: vec![user("hi", pi_host::now_ms())],
		..Context::default()
	}
}

fn object(value: Value) -> JsonObject {
	value.as_object().expect("an object literal").clone()
}

fn text(text: &str) -> AssistantContent {
	AssistantContent::Text(faux_text(text))
}

fn session(session_id: &str, cache_retention: CacheRetention) -> Option<SimpleStreamOptions> {
	Some(SimpleStreamOptions {
		base: StreamOptions {
			session_id: Some(session_id.to_string()),
			cache_retention: Some(cache_retention),
			..StreamOptions::default()
		},
		..SimpleStreamOptions::default()
	})
}

fn with_stop_reason(stop_reason: StopReason) -> Option<FauxAssistantMessageOptions> {
	Some(FauxAssistantMessageOptions {
		stop_reason: Some(stop_reason),
		..FauxAssistantMessageOptions::default()
	})
}

fn paced(tokens_per_second: f64, size: f64) -> Option<RegisterFauxProviderOptions> {
	Some(RegisterFauxProviderOptions {
		tokens_per_second: Some(tokens_per_second),
		token_size: Some(fixed_token_size(size)),
		..RegisterFauxProviderOptions::default()
	})
}

fn fixed_token_size(size: f64) -> FauxTokenSize {
	FauxTokenSize {
		min: Some(size),
		max: Some(size),
	}
}

fn with_signal(controller: &AbortController) -> Option<SimpleStreamOptions> {
	Some(SimpleStreamOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				signal: Some(controller.signal()),
				..ProviderRequestOptions::default()
			},
			..StreamOptions::default()
		},
		..SimpleStreamOptions::default()
	})
}

fn model_of(registration: &FauxProviderRegistration) -> Model {
	registration.get_model(None).expect("a faux registration has a model")
}

mod faux_provider {
	use super::*;

	#[test]
	fn registers_a_custom_provider_and_estimates_usage() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![faux_assistant_message("hello world", None).into()]);

			let context = Context {
				system_prompt: Some("Be concise.".into()),
				messages: vec![user("hi there", pi_host::now_ms())],
				..Context::default()
			};

			let response = complete(&model_of(&registration), &context, None).await.unwrap();
			assert_eq!(response.content, vec![text("hello world")]);
			assert!(response.usage.input > 0.0);
			assert!(response.usage.output > 0.0);
			assert_eq!(
				response.usage.total_tokens,
				response.usage.input + response.usage.output
			);
			assert_eq!(registration.state.borrow().call_count, 1);
		});
	}

	#[test]
	fn supports_helper_blocks_for_text_thinking_and_tool_calls() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message(
					vec![
						AssistantContent::Thinking(faux_thinking("think")),
						AssistantContent::ToolCall(faux_tool_call("echo", object(json!({ "text": "hi" })), None)),
						text("done"),
					],
					with_stop_reason(StopReason::ToolUse),
				)
				.into(),
			]);

			let response = complete(&model_of(&registration), &hi(), None).await.unwrap();

			assert_eq!(response.content.len(), 3);
			assert_eq!(
				response.content[0],
				AssistantContent::Thinking(ThinkingContent {
					thinking: "think".into(),
					..ThinkingContent::default()
				})
			);
			let AssistantContent::ToolCall(tool_call) = &response.content[1] else {
				panic!("expected a tool call, got {:?}", response.content[1]);
			};
			assert!(!tool_call.id.is_empty());
			assert_eq!(
				*tool_call,
				ToolCall {
					id: tool_call.id.clone(),
					name: "echo".into(),
					arguments: object(json!({ "text": "hi" })),
					..ToolCall::default()
				}
			);
			assert_eq!(response.content[2], text("done"));
			assert_eq!(response.stop_reason, StopReason::ToolUse);
		});
	}

	#[test]
	fn supports_multiple_models_with_per_model_reasoning_and_model_aware_factories() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(Some(RegisterFauxProviderOptions {
				models: Some(vec![
					FauxModelDefinition {
						id: "faux-fast".into(),
						name: Some("Faux Fast".into()),
						reasoning: Some(false),
						..FauxModelDefinition::default()
					},
					FauxModelDefinition {
						id: "faux-thinker".into(),
						name: Some("Faux Thinker".into()),
						reasoning: Some(true),
						..FauxModelDefinition::default()
					},
				]),
				..RegisterFauxProviderOptions::default()
			}));
			let factory = || -> FauxResponseStep {
				FauxResponseStep::Factory(Rc::new(|_context, _options, _state, model| {
					let message = faux_assistant_message(format!("{}:{}", model.id, model.reasoning), None);
					Box::pin(async move { Ok(message) })
				}))
			};
			registration.set_responses(vec![factory(), factory()]);

			assert_eq!(
				registration
					.models
					.iter()
					.map(|model| model.id.as_str())
					.collect::<Vec<_>>(),
				vec!["faux-fast", "faux-thinker"]
			);
			assert_eq!(registration.get_model(None).as_ref(), registration.models.first());
			assert!(!registration.get_model(Some("faux-fast")).unwrap().reasoning);
			assert!(registration.get_model(Some("faux-thinker")).unwrap().reasoning);

			let fast = complete(&registration.get_model(Some("faux-fast")).unwrap(), &hi(), None)
				.await
				.unwrap();
			let thinker = complete(&registration.get_model(Some("faux-thinker")).unwrap(), &hi(), None)
				.await
				.unwrap();

			assert_eq!(fast.content, vec![text("faux-fast:false")]);
			assert_eq!(thinker.content, vec![text("faux-thinker:true")]);
		});
	}

	#[test]
	fn rewrites_api_provider_and_model_on_returned_messages() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(Some(RegisterFauxProviderOptions {
				api: Some("faux:test".into()),
				provider: Some("faux-provider".into()),
				models: Some(vec![FauxModelDefinition {
					id: "faux-model".into(),
					..FauxModelDefinition::default()
				}]),
				..RegisterFauxProviderOptions::default()
			}));
			registration.set_responses(vec![faux_assistant_message("hello", None).into()]);

			let response = complete(&model_of(&registration), &hi(), None).await.unwrap();

			assert_eq!(response.api, "faux:test");
			assert_eq!(response.provider, "faux-provider");
			assert_eq!(response.model, "faux-model");
		});
	}

	#[test]
	fn consumes_queued_responses_in_order_and_errors_when_exhausted() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message("first", None).into(),
				faux_assistant_message("second", None).into(),
			]);

			let context = hi();

			let first = complete(&model_of(&registration), &context, None).await.unwrap();
			let second = complete(&model_of(&registration), &context, None).await.unwrap();
			let exhausted = complete(&model_of(&registration), &context, None).await.unwrap();

			assert_eq!(first.content, vec![text("first")]);
			assert_eq!(second.content, vec![text("second")]);
			assert_eq!(exhausted.stop_reason, StopReason::Error);
			assert_eq!(
				exhausted.error_message.as_deref(),
				Some("No more faux responses queued")
			);
			assert_eq!(registration.get_pending_response_count(), 0);
			assert_eq!(registration.state.borrow().call_count, 3);
		});
	}

	#[test]
	fn can_replace_and_append_queued_responses() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![faux_assistant_message("first", None).into()]);

			let context = hi();
			let model = model_of(&registration);

			assert_eq!(
				complete(&model, &context, None).await.unwrap().content,
				vec![text("first")]
			);
			assert_eq!(registration.get_pending_response_count(), 0);

			registration.set_responses(vec![faux_assistant_message("second", None).into()]);
			assert_eq!(registration.get_pending_response_count(), 1);
			assert_eq!(
				complete(&model, &context, None).await.unwrap().content,
				vec![text("second")]
			);

			registration.append_responses(vec![
				faux_assistant_message("third", None).into(),
				faux_assistant_message("fourth", None).into(),
			]);
			assert_eq!(registration.get_pending_response_count(), 2);
			assert_eq!(
				complete(&model, &context, None).await.unwrap().content,
				vec![text("third")]
			);
			assert_eq!(
				complete(&model, &context, None).await.unwrap().content,
				vec![text("fourth")]
			);
			assert_eq!(registration.get_pending_response_count(), 0);
		});
	}

	#[test]
	fn supports_async_response_factories() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![FauxResponseStep::Factory(Rc::new(
				|context, _options, state, _model| {
					let reply = format!("{}:{}", context.messages.len(), state.borrow().call_count);
					Box::pin(async move { Ok(faux_assistant_message(reply, None)) })
				},
			))]);

			let response = complete(&model_of(&registration), &hi(), None).await.unwrap();

			assert_eq!(response.content, vec![text("1:1")]);
		});
	}

	#[test]
	fn emits_an_error_when_a_response_factory_throws() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![FauxResponseStep::Factory(Rc::new(|_, _, _, _| {
				Box::pin(async { Err(PiError::new("boom")) })
			}))]);

			let events = collect_events(stream(&model_of(&registration), &hi(), None).unwrap()).await;

			assert_eq!(events.len(), 1);
			assert_eq!(event_type(&events[0]), "error");
			if let AssistantMessageEvent::Error { error, .. } = &events[0] {
				assert_eq!(error.borrow().stop_reason, StopReason::Error);
				assert_eq!(error.borrow().error_message.as_deref(), Some("boom"));
			}
		});
	}

	#[test]
	fn rejects_a_queued_response_without_a_terminal_stop_reason() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message("partial", with_stop_reason(StopReason::Pending)).into(),
			]);

			let events = collect_events(stream(&model_of(&registration), &hi(), None).unwrap()).await;

			assert!(!events.iter().any(|event| event_type(event) == "done"));
			let terminal = events.last();
			assert_eq!(terminal.map(event_type), Some("error"));
			if let Some(AssistantMessageEvent::Error { error, .. }) = terminal {
				assert_eq!(error.borrow().stop_reason, StopReason::Error);
				assert_eq!(
					error.borrow().error_message.as_deref(),
					Some("Faux response ended without a stop reason")
				);
			}
		});
	}

	#[test]
	fn estimates_prompt_and_output_tokens_from_serialized_context() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![faux_assistant_message("done", None).into()]);

			let tool = Tool {
				name: "echo".into(),
				description: "Echo back text".into(),
				parameters: json!({
					"type": "object",
					"properties": { "text": { "type": "string" } },
					"required": ["text"],
				}),
				..Tool::default()
			};
			let context = Context {
				system_prompt: Some("sys".into()),
				messages: vec![
					Message::User(UserMessage {
						content: UserMessageContent::Blocks(vec![
							UserContent::Text(TextContent {
								text: "hello".into(),
								..TextContent::default()
							}),
							UserContent::Image(ImageContent {
								mime_type: "image/png".into(),
								data: "abcd".into(),
								..ImageContent::default()
							}),
						]),
						timestamp: 1.0,
						..UserMessage::default()
					}),
					Message::Assistant(faux_assistant_message("prior", None)),
					Message::ToolResult(ToolResultMessage {
						tool_call_id: "tool-1".into(),
						tool_name: "echo".into(),
						content: vec![UserContent::Text(TextContent {
							text: "tool out".into(),
							..TextContent::default()
						})],
						is_error: false,
						timestamp: 2.0,
						..ToolResultMessage::default()
					}),
				],
				tools: Some(vec![tool.clone()]),
			};

			let response = complete(&model_of(&registration), &context, None).await.unwrap();
			let prompt_text = [
				"system:sys".to_string(),
				"user:hello\n[image:image/png:4]".to_string(),
				"assistant:prior".to_string(),
				"toolResult:echo\ntool out".to_string(),
				format!("tools:{}", pi_host::js::json_stringify(&vec![tool])),
			]
			.join("\n\n");
			let expected_prompt_tokens = (utf16_len(&prompt_text) as f64 / 4.0).ceil();
			let expected_output_tokens = (utf16_len("done") as f64 / 4.0).ceil();

			assert_eq!(response.usage.input, expected_prompt_tokens);
			assert_eq!(response.usage.output, expected_output_tokens);
			assert_eq!(response.usage.cache_read, 0.0);
			assert_eq!(response.usage.cache_write, 0.0);
			assert_eq!(
				response.usage.total_tokens,
				expected_prompt_tokens + expected_output_tokens
			);
		});
	}

	#[test]
	fn does_not_share_cache_across_sessions_or_requests_without_session_id() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message("first", None).into(),
				faux_assistant_message("second", None).into(),
				faux_assistant_message("third", None).into(),
			]);

			let mut context = Context {
				messages: vec![user("hello", pi_host::now_ms())],
				..Context::default()
			};
			let model = model_of(&registration);

			let first = complete(&model, &context, session("session-1", CacheRetention::Short))
				.await
				.unwrap();
			assert!(first.usage.cache_write > 0.0);
			context.messages.push(Message::Assistant(first));
			context.messages.push(user("follow up", pi_host::now_ms() + 1.0));

			let second = complete(&model, &context, session("session-2", CacheRetention::Short))
				.await
				.unwrap();
			assert_eq!(second.usage.cache_read, 0.0);
			assert!(second.usage.cache_write > 0.0);

			let third = complete(&model, &context, None).await.unwrap();
			assert_eq!(third.usage.cache_read, 0.0);
			assert_eq!(third.usage.cache_write, 0.0);
		});
	}

	#[test]
	fn simulates_prompt_caching_per_session_id() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message("first", None).into(),
				faux_assistant_message("second", None).into(),
			]);

			let mut context = Context {
				system_prompt: Some("Be concise.".into()),
				messages: vec![user("hello", pi_host::now_ms())],
				..Context::default()
			};
			let model = model_of(&registration);

			let first = complete(&model, &context, session("session-1", CacheRetention::Short))
				.await
				.unwrap();
			assert_eq!(first.usage.cache_read, 0.0);
			assert!(first.usage.cache_write > 0.0);

			context.messages.push(Message::Assistant(first));
			context.messages.push(user("follow up", pi_host::now_ms() + 1.0));

			let second = complete(&model, &context, session("session-1", CacheRetention::Short))
				.await
				.unwrap();
			assert!(second.usage.cache_read > 0.0);
			assert!(second.usage.input + second.usage.cache_read > second.usage.input);
		});
	}

	#[test]
	fn counts_cached_characters_up_to_the_first_difference_in_the_joined_prompt() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message("a", None).into(),
				faux_assistant_message("b", None).into(),
				faux_assistant_message("c", None).into(),
			]);
			let options = || session("session-1", CacheRetention::Short);
			let context = |messages: &[&str]| Context {
				messages: messages.iter().map(|content| user(content, 1.0)).collect(),
				..Context::default()
			};
			let model = model_of(&registration);
			let usage =
				|message: &AssistantMessage| (message.usage.input, message.usage.cache_read, message.usage.cache_write);

			// Prompt texts: "user:hello world" (16 characters), then 16 + 2 + "user:next" (9) = 27.
			let first = complete(&model, &context(&["hello world"]), options()).await.unwrap();
			assert_eq!(usage(&first), (0.0, 0.0, 4.0));
			let extended = complete(&model, &context(&["hello world", "next"]), options())
				.await
				.unwrap();
			assert_eq!(usage(&extended), (0.0, 4.0, 3.0));

			// The first message now differs after "user:hello w" (12 characters): "user:hello wide" + 2 + 9 = 26.
			let edited = complete(&model, &context(&["hello wide", "next"]), options())
				.await
				.unwrap();
			assert_eq!(usage(&edited), (0.0, 3.0, 4.0));
		});
	}

	#[test]
	fn does_not_simulate_caching_when_cache_retention_is_none() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message("first", None).into(),
				faux_assistant_message("second", None).into(),
			]);

			let mut context = Context {
				messages: vec![user("hello", pi_host::now_ms())],
				..Context::default()
			};
			let model = model_of(&registration);

			complete(&model, &context, session("session-1", CacheRetention::None))
				.await
				.unwrap();
			context
				.messages
				.push(Message::Assistant(faux_assistant_message("first", None)));
			context.messages.push(user("follow up", pi_host::now_ms() + 1.0));
			let second = complete(&model, &context, session("session-1", CacheRetention::None))
				.await
				.unwrap();
			assert_eq!(second.usage.cache_read, 0.0);
			assert_eq!(second.usage.cache_write, 0.0);
		});
	}

	#[test]
	fn streams_thinking_text_and_partial_tool_call_deltas() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![
				faux_assistant_message(
					vec![
						AssistantContent::Thinking(faux_thinking("thinking text")),
						text("answer text"),
						AssistantContent::ToolCall(faux_tool_call(
							"echo",
							object(json!({ "text": "hi", "count": 12 })),
							Some(FauxToolCallOptions {
								id: Some("tool-1".into()),
							}),
						)),
					],
					with_stop_reason(StopReason::ToolUse),
				)
				.into(),
			]);

			let mut events = Vec::new();
			let mut tool_call_deltas = Vec::new();
			let s = stream(&model_of(&registration), &hi(), None).unwrap();
			while let Some(event) = s.next().await {
				events.push(event_type(&event));
				if let AssistantMessageEvent::ToolcallDelta { delta, .. } = &event {
					tool_call_deltas.push(delta.clone());
				}
			}

			assert!(events.contains(&"thinking_start"));
			assert!(events.contains(&"thinking_delta"));
			assert!(events.contains(&"text_start"));
			assert!(events.contains(&"text_delta"));
			assert!(events.contains(&"toolcall_start"));
			assert!(events.contains(&"toolcall_delta"));
			assert!(events.contains(&"toolcall_end"));
			assert!(tool_call_deltas.len() > 1);
			assert_eq!(
				serde_json::from_str::<Value>(&tool_call_deltas.join("")).unwrap(),
				json!({ "text": "hi", "count": 12 })
			);
		});
	}

	#[test]
	fn streams_an_exact_event_order_for_fixed_size_chunks() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(Some(RegisterFauxProviderOptions {
				token_size: Some(fixed_token_size(1.0)),
				..RegisterFauxProviderOptions::default()
			}));
			registration.set_responses(vec![
				faux_assistant_message(
					vec![
						AssistantContent::Thinking(faux_thinking("go")),
						text("ok"),
						AssistantContent::ToolCall(faux_tool_call(
							"echo",
							JsonObject::new(),
							Some(FauxToolCallOptions {
								id: Some("tool-1".into()),
							}),
						)),
					],
					with_stop_reason(StopReason::ToolUse),
				)
				.into(),
			]);

			let events = collect_events(stream(&model_of(&registration), &hi(), None).unwrap()).await;

			let AssistantMessageEvent::Start { partial } = &events[0] else {
				panic!("expected start, got {:?}", events[0]);
			};
			assert_eq!(partial.borrow().stop_reason, StopReason::Pending);
			assert_eq!(
				events.iter().map(event_type).collect::<Vec<_>>(),
				vec![
					"start",
					"thinking_start",
					"thinking_delta",
					"thinking_end",
					"text_start",
					"text_delta",
					"text_end",
					"toolcall_start",
					"toolcall_delta",
					"toolcall_end",
					"done",
				]
			);
		});
	}

	#[test]
	fn streams_multiple_tool_calls_in_one_message() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			let tool_call = |text: &str, id: &str| {
				AssistantContent::ToolCall(faux_tool_call(
					"echo",
					object(json!({ "text": text })),
					Some(FauxToolCallOptions { id: Some(id.into()) }),
				))
			};
			registration.set_responses(vec![
				faux_assistant_message(
					vec![tool_call("one", "tool-1"), tool_call("two", "tool-2")],
					with_stop_reason(StopReason::ToolUse),
				)
				.into(),
			]);

			let events = collect_events(stream(&model_of(&registration), &hi(), None).unwrap()).await;

			assert_eq!(
				events
					.iter()
					.filter(|event| event_type(event) == "toolcall_start")
					.count(),
				2
			);
			assert_eq!(
				events
					.iter()
					.filter(|event| event_type(event) == "toolcall_end")
					.count(),
				2
			);
		});
	}

	#[test]
	fn streams_an_explicit_assistant_error_message_as_a_terminal_error() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(Some(RegisterFauxProviderOptions {
				token_size: Some(fixed_token_size(2.0)),
				..RegisterFauxProviderOptions::default()
			}));
			registration.set_responses(vec![
				AssistantMessage {
					stop_reason: StopReason::Error,
					error_message: Some("upstream failed".into()),
					..faux_assistant_message("partial", None)
				}
				.into(),
			]);

			let events = collect_events(stream(&model_of(&registration), &hi(), None).unwrap()).await;

			assert_eq!(
				events.iter().map(event_type).collect::<Vec<_>>(),
				vec!["start", "text_start", "text_delta", "text_end", "error"]
			);
			let terminal = &events[events.len() - 1];
			assert_eq!(event_type(terminal), "error");
			if let AssistantMessageEvent::Error { reason, error } = terminal {
				assert_eq!(*reason, StopReason::Error);
				assert_eq!(error.borrow().stop_reason, StopReason::Error);
				assert_eq!(error.borrow().error_message.as_deref(), Some("upstream failed"));
			}
		});
	}

	#[test]
	fn streams_an_explicit_assistant_aborted_message_as_a_terminal_error() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(Some(RegisterFauxProviderOptions {
				token_size: Some(fixed_token_size(2.0)),
				..RegisterFauxProviderOptions::default()
			}));
			registration.set_responses(vec![
				AssistantMessage {
					stop_reason: StopReason::Aborted,
					error_message: Some("Request was aborted".into()),
					..faux_assistant_message("partial", None)
				}
				.into(),
			]);

			let events = collect_events(stream(&model_of(&registration), &hi(), None).unwrap()).await;

			assert_eq!(
				events.iter().map(event_type).collect::<Vec<_>>(),
				vec!["start", "text_start", "text_delta", "text_end", "error"]
			);
			let terminal = &events[events.len() - 1];
			assert_eq!(event_type(terminal), "error");
			if let AssistantMessageEvent::Error { reason, error } = terminal {
				assert_eq!(*reason, StopReason::Aborted);
				assert_eq!(error.borrow().stop_reason, StopReason::Aborted);
				assert_eq!(error.borrow().error_message.as_deref(), Some("Request was aborted"));
			}
		});
	}

	#[test]
	fn supports_aborting_before_the_first_chunk() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(paced(50.0, 3.0));
			registration.set_responses(vec![faux_assistant_message("abcdefghijklmnopqrstuvwxyz", None).into()]);

			let controller = AbortController::new();
			controller.abort();
			let events =
				collect_events(stream(&model_of(&registration), &hi(), with_signal(&controller)).unwrap()).await;

			assert_eq!(events.len(), 1);
			assert_eq!(event_type(&events[0]), "error");
			if let AssistantMessageEvent::Error { reason, error } = &events[0] {
				assert_eq!(*reason, StopReason::Aborted);
				assert_eq!(error.borrow().stop_reason, StopReason::Aborted);
			}
		});
	}

	#[test]
	fn supports_aborting_mid_text_stream_when_paced() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(paced(100.0, 3.0));
			registration.set_responses(vec![faux_assistant_message("abcdefghijklmnopqrstuvwxyz", None).into()]);

			let controller = AbortController::new();
			let mut events = Vec::new();
			let mut text_delta_count = 0;
			let s = stream(&model_of(&registration), &hi(), with_signal(&controller)).unwrap();
			while let Some(event) = s.next().await {
				events.push(event_type(&event));
				if event_type(&event) == "text_delta" {
					text_delta_count += 1;
					controller.abort();
				}
			}

			assert_eq!(text_delta_count, 1);
			assert!(events.contains(&"text_start"));
			assert!(events.contains(&"text_delta"));
			assert!(events.contains(&"error"));
			assert!(!events.contains(&"text_end"));
		});
	}

	#[test]
	fn supports_aborting_mid_thinking_stream_when_paced() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(paced(100.0, 3.0));
			registration.set_responses(vec![
				AssistantMessage {
					content: vec![AssistantContent::Thinking(faux_thinking("abcdefghijklmnopqrstuvwxyz"))],
					..faux_assistant_message("ignored", None)
				}
				.into(),
			]);

			let controller = AbortController::new();
			let mut events = Vec::new();
			let mut thinking_delta_count = 0;
			let s = stream(&model_of(&registration), &hi(), with_signal(&controller)).unwrap();
			while let Some(event) = s.next().await {
				events.push(event_type(&event));
				if event_type(&event) == "thinking_delta" {
					thinking_delta_count += 1;
					controller.abort();
				}
			}

			assert_eq!(thinking_delta_count, 1);
			assert!(events.contains(&"thinking_start"));
			assert!(events.contains(&"thinking_delta"));
			assert!(events.contains(&"error"));
			assert!(!events.contains(&"thinking_end"));
		});
	}

	#[test]
	fn supports_aborting_mid_toolcall_stream_when_paced() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(paced(100.0, 3.0));
			registration.set_responses(vec![
				AssistantMessage {
					content: vec![AssistantContent::ToolCall(ToolCall {
						id: "tool-1".into(),
						name: "echo".into(),
						arguments: object(json!({ "text": "abcdefghijklmnopqrstuvwxyz", "count": 123456789 })),
						..ToolCall::default()
					})],
					stop_reason: StopReason::ToolUse,
					..faux_assistant_message("done", None)
				}
				.into(),
			]);

			let controller = AbortController::new();
			let mut events = Vec::new();
			let mut tool_call_delta_count = 0;
			let s = stream(&model_of(&registration), &hi(), with_signal(&controller)).unwrap();
			while let Some(event) = s.next().await {
				events.push(event_type(&event));
				if event_type(&event) == "toolcall_delta" {
					tool_call_delta_count += 1;
					controller.abort();
				}
			}

			assert_eq!(tool_call_delta_count, 1);
			assert!(events.contains(&"toolcall_start"));
			assert!(events.contains(&"toolcall_delta"));
			assert!(events.contains(&"error"));
			assert!(!events.contains(&"toolcall_end"));
		});
	}

	#[test]
	fn unregisters_the_provider() {
		pi_host::testing::run(|_| async {
			let registration = register_faux_provider(None);
			registration.set_responses(vec![faux_assistant_message("hello", None).into()]);
			registration.unregister();

			let error = complete(&model_of(&registration), &hi(), None).await.unwrap_err();
			assert_eq!(
				error.message,
				format!("No API provider registered for api: {}", registration.api)
			);
		});
	}
}

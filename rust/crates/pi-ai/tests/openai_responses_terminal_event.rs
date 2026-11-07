//! Port of `packages/ai/test/openai-responses-terminal-event.test.ts`.
//!
//! `vi.mock("openai")` is replaced by the test host answering the request with the same events as SSE. The
//! `stream.push` spy of the phase cases is a task that consumes the event stream while the provider events are
//! processed one at a time (`yielding_events`), recording each event's `partial.stopReason` as it arrives. The mocked
//! response body arrives on a later timer tick (`delayed_body`), as a network body would, so `for await` sees the
//! `start` event before the stream fails.

mod openai_responses_support;

use std::cell::RefCell;
use std::rc::Rc;

use openai_responses_support::{
	assert_match_object, context, create_output, delayed_body, events, gpt5_mini, response, responses_options,
	sse_body, yield_now, yielding_events,
};
use pi_ai::api::openai_responses::stream as stream_open_ai_responses;
use pi_ai::api::openai_responses_shared::process_responses_stream;
use pi_ai::types::{AssistantMessageEvent, Model, OnProviderStreamEvent, StopReason};
use pi_ai::utils::event_stream::AssistantMessageEventStream;
use pi_host::testing::{TestHost, run};
use serde_json::{Value, json};

fn mock_openai(host: &TestHost) {
	let body = sse_body(&[
		json!({ "type": "response.created", "sequence_number": 0, "response": { "id": "resp_wrapper_early_eof" } }),
		json!({
			"type": "response.output_item.added",
			"sequence_number": 1,
			"output_index": 0,
			"item": { "type": "reasoning", "id": "rs_wrapper_early_eof", "summary": [] },
		}),
		json!({
			"type": "response.reasoning_text.delta",
			"sequence_number": 2,
			"output_index": 0,
			"content_index": 0,
			"item_id": "rs_wrapper_early_eof",
			"delta": "partial reasoning before the wrapper stream ends",
		}),
	]);
	host.on_fetch(move |_| {
		let body = body.clone();
		Box::pin(async move {
			Ok(delayed_body(response(
				200,
				&[("content-type", "text/event-stream")],
				body,
			)))
		})
	});
}

fn create_early_eof_events() -> Vec<Value> {
	vec![
		json!({ "type": "response.created", "sequence_number": 0, "response": { "id": "resp_early_eof" } }),
		json!({
			"type": "response.output_item.added",
			"sequence_number": 1,
			"output_index": 0,
			"item": { "type": "reasoning", "id": "rs_early_eof", "summary": [] },
		}),
		json!({
			"type": "response.reasoning_text.delta",
			"sequence_number": 2,
			"output_index": 0,
			"content_index": 0,
			"item_id": "rs_early_eof",
			"delta": "partial reasoning before the stream ends",
		}),
	]
}

fn create_completed_events() -> Vec<Value> {
	vec![json!({
		"type": "response.completed",
		"sequence_number": 0,
		"response": {
			"id": "resp_completed",
			"status": "completed",
			"usage": {
				"input_tokens": 20,
				"output_tokens": 7,
				"total_tokens": 27,
				"input_tokens_details": { "cached_tokens": 2, "cache_write_tokens": 3 },
			},
		},
	})]
}

fn create_incomplete_events(reason: &str) -> Vec<Value> {
	vec![json!({
		"type": "response.incomplete",
		"sequence_number": 0,
		"response": {
			"id": "resp_incomplete",
			"status": "incomplete",
			"incomplete_details": { "reason": reason },
			"usage": {
				"input_tokens": 30,
				"output_tokens": 12,
				"total_tokens": 42,
				"input_tokens_details": { "cached_tokens": 5 },
			},
		},
	})]
}

fn create_failed_events() -> Vec<Value> {
	vec![json!({
		"type": "response.failed",
		"sequence_number": 0,
		"response": { "id": "resp_failed", "status": "failed", "error": { "code": "server_error", "message": "boom" } },
	})]
}

fn create_phased_message_events(phases: [&str; 2], terminal_status: &str) -> Vec<Value> {
	let mut events = vec![
		json!({
			"type": "response.output_item.added",
			"sequence_number": 0,
			"output_index": 0,
			"item": {
				"type": "message",
				"id": "msg_phase",
				"role": "assistant",
				"status": "in_progress",
				"content": [],
				"phase": phases[0],
			},
		}),
		json!({
			"type": "response.output_item.done",
			"sequence_number": 1,
			"output_index": 0,
			"item": {
				"type": "message",
				"id": "msg_phase",
				"role": "assistant",
				"status": "completed",
				"content": [{ "type": "output_text", "text": "answer", "annotations": [] }],
				"phase": phases[1],
			},
		}),
	];
	if terminal_status == "incomplete" {
		events.push(json!({
			"type": "response.incomplete",
			"sequence_number": 2,
			"response": {
				"id": "resp_phase",
				"status": "incomplete",
				"incomplete_details": { "reason": "max_output_tokens" },
			},
		}));
		return events;
	}
	events.push(json!({
		"type": "response.completed",
		"sequence_number": 2,
		"response": { "id": "resp_phase", "status": "completed" },
	}));
	events
}

fn create_unfinished_tool_call_events() -> Vec<Value> {
	vec![
		json!({
			"type": "response.output_item.added",
			"sequence_number": 0,
			"output_index": 0,
			"item": { "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "bash", "arguments": "" },
		}),
		json!({
			"type": "response.function_call_arguments.delta",
			"sequence_number": 1,
			"output_index": 0,
			"item_id": "fc_1",
			"delta": "{\"command\":\"rm -rf /tmp/build",
		}),
		json!({
			"type": "response.completed",
			"sequence_number": 2,
			"response": { "id": "resp_unfinished", "status": "completed" },
		}),
	]
}

// llama.cpp omits output_index from every event and sends both done events after all deltas.
fn create_tool_calls_without_output_index_events() -> Vec<Value> {
	let call = |n: &str, arguments: &str| json!({ "type": "function_call", "id": format!("fc_{n}"), "call_id": format!("call_{n}"), "name": "bash", "arguments": arguments });
	vec![
		json!({ "type": "response.output_item.added", "item": call("a", "") }),
		json!({ "type": "response.function_call_arguments.delta", "item_id": "fc_a", "delta": "{\"command\":\"echo a\"}" }),
		json!({ "type": "response.output_item.added", "item": call("b", "") }),
		json!({ "type": "response.function_call_arguments.delta", "item_id": "fc_b", "delta": "{\"command\":\"echo b\"}" }),
		json!({ "type": "response.output_item.done", "item": call("a", "{\"command\":\"echo a\"}") }),
		json!({ "type": "response.output_item.done", "item": call("b", "{\"command\":\"echo b\"}") }),
		json!({ "type": "response.completed", "response": { "id": "resp_no_output_index", "status": "completed" } }),
	]
}

fn hi_context() -> pi_ai::types::TranscriptContext {
	context(json!({
		"systemPrompt": "",
		"messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }], "timestamp": 0 }],
		"tools": [],
	}))
}

async fn process_error(events_list: Vec<Value>) -> String {
	let model = gpt5_mini();
	process_responses_stream(
		events(events_list),
		&create_output(&model),
		&AssistantMessageEventStream::new(),
		&model,
		None,
	)
	.await
	.expect_err("processResponsesStream rejects")
	.message
}

/// Processes `events_list` while a consumer records each event's `partial.stopReason`, like the `stream.push` spy.
async fn observe_stop_reasons(events_list: Vec<Value>) -> (Vec<StopReason>, StopReason) {
	let model = gpt5_mini();
	let output = create_output(&model);
	let stream = AssistantMessageEventStream::new();
	let observed = Rc::new(RefCell::new(Vec::new()));
	let (consumer, target) = (stream.clone(), observed.clone());
	pi_host::spawn_local(async move {
		while let Some(event) = consumer.next().await {
			if let Some(partial) = partial_of(&event) {
				target.borrow_mut().push(partial.borrow().stop_reason);
			}
		}
	});

	process_responses_stream(yielding_events(events_list), &output, &stream, &model, None)
		.await
		.unwrap();
	yield_now().await;
	let observed = observed.borrow().clone();
	let stop_reason = output.borrow().stop_reason;
	(observed, stop_reason)
}

fn partial_of(event: &AssistantMessageEvent) -> Option<&pi_ai::types::SharedAssistantMessage> {
	match event {
		AssistantMessageEvent::Start { partial }
		| AssistantMessageEvent::TextStart { partial, .. }
		| AssistantMessageEvent::TextDelta { partial, .. }
		| AssistantMessageEvent::TextEnd { partial, .. }
		| AssistantMessageEvent::ThinkingStart { partial, .. }
		| AssistantMessageEvent::ThinkingDelta { partial, .. }
		| AssistantMessageEvent::ThinkingEnd { partial, .. }
		| AssistantMessageEvent::ToolcallStart { partial, .. }
		| AssistantMessageEvent::ToolcallDelta { partial, .. }
		| AssistantMessageEvent::ToolcallEnd { partial, .. } => Some(partial),
		_ => None,
	}
}

mod open_ai_responses_terminal_event_handling {
	use super::*;

	#[test]
	fn rejects_streams_that_end_before_a_terminal_response_event() {
		run(|_| async {
			assert_eq!(
				process_error(create_early_eof_events()).await,
				"OpenAI Responses stream ended before a terminal response event"
			);
		});
	}

	#[test]
	fn rejects_completed_streams_whose_tool_call_never_received_output_item_done() {
		run(|_| async {
			assert!(
				process_error(create_unfinished_tool_call_events())
					.await
					.contains("OpenAI Responses stream completed with an unfinished tool call: bash (call_1|fc_1)")
			);
		});
	}

	// https://github.com/earendil-works/pi/issues/9974
	#[test]
	fn rejects_parallel_tool_calls_without_output_index_instead_of_running_mixed_up_calls() {
		run(|_| async {
			assert!(
				process_error(create_tool_calls_without_output_index_events())
					.await
					.contains("OpenAI Responses stream completed with an unfinished tool call: bash (call_a|fc_a)")
			);
		});
	}

	#[test]
	fn forwards_parsed_provider_stream_events_in_order() {
		run(|host| async move {
			mock_openai(&host);
			let model = gpt5_mini();
			let provider_events: Rc<RefCell<Vec<Value>>> = Rc::default();
			let event_models: Rc<RefCell<Vec<Model>>> = Rc::default();
			let (events_target, models_target) = (provider_events.clone(), event_models.clone());
			let on_provider_stream_event: OnProviderStreamEvent = Rc::new(move |event, event_model| {
				let (event, event_model) = (event.clone(), event_model.clone());
				let (events_target, models_target) = (events_target.clone(), models_target.clone());
				Box::pin(async move {
					yield_now().await;
					events_target.borrow_mut().push(event);
					models_target.borrow_mut().push(event_model);
					Ok(())
				})
			});
			let options = responses_options(|options| {
				options.api_key = Some("test".into());
				options.on_provider_stream_event = Some(on_provider_stream_event);
			});
			let stream = stream_open_ai_responses(&model, &hi_context(), Some(options));

			stream.result().await;

			let provider_events = provider_events.borrow();
			assert_eq!(provider_events.len(), 3);
			assert_eq!(
				provider_events
					.iter()
					.map(|event| event["type"].clone())
					.collect::<Vec<_>>(),
				[
					"response.created",
					"response.output_item.added",
					"response.reasoning_text.delta"
				]
			);
			assert_eq!(*event_models.borrow(), [model.clone(), model.clone(), model]);
		});
	}

	#[test]
	fn emits_an_error_final_result_when_the_wrapper_stream_ends_before_a_terminal_response_event() {
		run(|host| async move {
			mock_openai(&host);
			let options = responses_options(|options| {
				options.api_key = Some("test".into());
			});
			let stream = stream_open_ai_responses(&gpt5_mini(), &hi_context(), Some(options));
			let mut last_event = None;
			let mut initial_stop_reason = None;

			while let Some(event) = stream.next().await {
				if let AssistantMessageEvent::Start { partial } = &event {
					initial_stop_reason = Some(partial.borrow().stop_reason);
				}
				last_event = Some(event);
			}

			let result = stream.result().await.expect("stream has a result");
			assert_eq!(initial_stop_reason, Some(StopReason::Pending));
			assert!(matches!(last_event, Some(AssistantMessageEvent::Error { .. })));
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("OpenAI Responses stream ended before a terminal response event")
			);
		});
	}

	#[test]
	fn tracks_message_phases_phases() {
		run(|_| async {
			let cases = [
				(["commentary", "commentary"], [StopReason::Pending, StopReason::Pending]),
				(["final_answer", "final_answer"], [StopReason::Stop, StopReason::Stop]),
				(["commentary", "final_answer"], [StopReason::Pending, StopReason::Stop]),
			];
			for (phases, expected) in cases {
				let (observed, stop_reason) =
					observe_stop_reasons(create_phased_message_events(phases, "completed")).await;

				assert_eq!(observed, expected, "{phases:?}");
				assert_eq!(stop_reason, StopReason::Stop, "{phases:?}");
			}
		});
	}

	#[test]
	fn replaces_a_provisional_final_answer_stop_with_an_incomplete_terminal_reason() {
		run(|_| async {
			let (observed, stop_reason) = observe_stop_reasons(create_phased_message_events(
				["final_answer", "final_answer"],
				"incomplete",
			))
			.await;

			assert_eq!(observed, [StopReason::Stop, StopReason::Stop]);
			assert_eq!(stop_reason, StopReason::Length);
		});
	}

	#[test]
	fn finalizes_completed_terminal_events_as_stop() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			process_responses_stream(
				events(create_completed_events()),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.unwrap();

			let output = output.borrow();
			assert_eq!(output.response_id.as_deref(), Some("resp_completed"));
			assert_eq!(output.stop_reason, StopReason::Stop);
			assert_eq!(output.raw_stop_reason.as_deref(), Some("completed"));
			assert_match_object(
				&serde_json::to_value(&output.usage).unwrap(),
				&json!({ "input": 15, "output": 7, "cacheRead": 2, "cacheWrite": 3, "totalTokens": 27 }),
			);
		});
	}

	#[test]
	fn finalizes_incomplete_terminal_events_as_length_stops() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			process_responses_stream(
				events(create_incomplete_events("max_output_tokens")),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.unwrap();

			let output = output.borrow();
			assert_eq!(output.response_id.as_deref(), Some("resp_incomplete"));
			assert_eq!(output.stop_reason, StopReason::Length);
			assert_eq!(output.raw_stop_reason.as_deref(), Some("incomplete.max_output_tokens"));
			assert_match_object(
				&serde_json::to_value(&output.usage).unwrap(),
				&json!({ "input": 25, "output": 12, "cacheRead": 5, "cacheWrite": 0, "totalTokens": 42 }),
			);
		});
	}

	#[test]
	fn finalizes_content_filtered_incomplete_responses_as_non_retryable_errors() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			process_responses_stream(
				events(create_incomplete_events("content_filter")),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.unwrap();

			let output = output.borrow();
			assert_eq!(output.stop_reason, StopReason::Error);
			assert_eq!(output.raw_stop_reason.as_deref(), Some("incomplete.content_filter"));
			assert_eq!(
				output.error_message.as_deref(),
				Some("Response incomplete: content_filter")
			);
		});
	}

	#[test]
	fn preserves_unknown_provider_incomplete_reasons_as_non_retryable_errors() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			process_responses_stream(
				events(create_incomplete_events("max_time_limit")),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.unwrap();

			let output = output.borrow();
			assert_eq!(output.stop_reason, StopReason::Error);
			assert_eq!(output.raw_stop_reason.as_deref(), Some("incomplete.max_time_limit"));
			assert_eq!(
				output.error_message.as_deref(),
				Some("Response incomplete: max_time_limit")
			);
		});
	}

	#[test]
	fn rejects_failed_terminal_events_with_the_provider_error() {
		run(|_| async {
			let model = gpt5_mini();
			let output = create_output(&model);
			let error = process_responses_stream(
				events(create_failed_events()),
				&output,
				&AssistantMessageEventStream::new(),
				&model,
				None,
			)
			.await
			.expect_err("processResponsesStream rejects");

			assert!(error.message.contains("server_error: boom"), "{}", error.message);
			assert_eq!(output.borrow().raw_stop_reason.as_deref(), Some("failed"));
		});
	}
}

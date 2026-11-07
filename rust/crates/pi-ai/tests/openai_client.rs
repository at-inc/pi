//! Tests of `api/openai_client.rs`, the Rust replacement of the `openai` npm SDK. There is no vitest file: each case
//! checks one SDK behavior pi-ai observes through `openai-completions` (or, in `non_stream_payloads`, how
//! `openai-completions` iterates what the SDK returns), with the expected values taken from the TS `stream` run against
//! the real SDK (version 7.19.0) and a stubbed `fetch`.

mod openai_completions_support;

use indexmap::IndexMap;
use openai_completions_support::{context, model, options, sse_body, user_message};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::{AssistantContent, StopReason};
use pi_host::HttpResponse;
use pi_host::testing::run;
use serde_json::json;

fn text_chunk() -> serde_json::Value {
	json!({ "id": "c", "choices": [{ "delta": { "content": "hi" }, "finish_reason": "stop" }] })
}

mod request {
	use super::*;

	#[test]
	fn builds_the_url_headers_and_body_of_a_chat_completion() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async {
					Ok(HttpResponse::from_bytes(
						200,
						IndexMap::new(),
						sse_body(&[text_chunk()]),
					))
				})
			});
			let mut request = options("k");
			request.timeout_ms = Some(1500.0);
			request.headers = Some(IndexMap::from([
				("X-Empty".to_string(), Some(String::new())),
				("Accept".to_string(), Some(" text/plain ".to_string())),
				("X-Null".to_string(), None),
			]));
			let model = model("m", "custom", "https://example.test/v1/?a=1&b=x%20y");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(request))
				.result()
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Stop);

			let sent = host.requests().pop().expect("a request was sent");
			assert_eq!(sent.method, "POST");
			assert_eq!(sent.url, "https://example.test/v1/?a=1&b=x%20y%2Fchat%2Fcompletions");
			let headers: Vec<(&str, &str)> =
				sent.headers
					.iter()
					.filter(|(name, _)| {
						!matches!(
							name.as_str(),
							"user-agent"
								| "x-stainless-arch" | "x-stainless-os"
								| "x-stainless-runtime" | "x-stainless-runtime-version"
						)
					})
					.map(|(name, value)| (name.as_str(), value.as_str()))
					.collect();
			assert_eq!(
				headers,
				vec![
					("accept", "text/plain"),
					("authorization", "Bearer k"),
					("content-type", "application/json"),
					("x-empty", ""),
					("x-stainless-lang", "js"),
					("x-stainless-package-version", "7.19.0"),
					("x-stainless-retry-count", "0"),
					("x-stainless-timeout", "1"),
				]
			);
			assert_eq!(
				sent.body.as_deref(),
				Some(
					r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true,"stream_options":{"include_usage":true},"store":false}"#
				)
			);
		});
	}
}

mod sse_response {
	use super::*;

	#[test]
	fn fails_a_stream_whose_response_has_a_null_body_status() {
		run(|host| async move {
			host.on_fetch(|_| Box::pin(async { Ok(HttpResponse::from_bytes(204, IndexMap::new(), "")) }));
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(options("k")))
				.result()
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Attempted to iterate over a response with no body")
			);
		});
	}

	#[test]
	fn skips_an_event_with_an_empty_event_name_and_no_data() {
		run(|host| async move {
			host.on_fetch(|_| {
				let body = format!("event:\n\n{}", sse_body(&[text_chunk()]));
				Box::pin(async move { Ok(HttpResponse::from_bytes(200, IndexMap::new(), body)) })
			});
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(options("k")))
				.result()
				.await
				.unwrap();
			assert_eq!(result.error_message, None);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert!(matches!(&result.content[..], [AssistantContent::Text(text)] if text.text == "hi"));
		});
	}
}

mod transport {
	use super::*;
	use futures::StreamExt;
	use pi_host::{AbortController, PiError};

	#[test]
	fn fails_with_request_timed_out_when_the_headers_do_not_arrive_in_time() {
		run(|host| async move {
			host.on_fetch(|request| {
				Box::pin(async move {
					request
						.signal
						.expect("the client passes a signal")
						.aborted_future()
						.await;
					Err(PiError::abort())
				})
			});
			let mut request = options("k");
			request.timeout_ms = Some(50.0);
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(request))
				.result()
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(result.error_message.as_deref(), Some("Request timed out."));
		});
	}

	#[test]
	fn ends_the_body_quietly_when_the_caller_aborts_mid_stream() {
		run(|host| async move {
			let controller = AbortController::new();
			let signal = controller.signal();
			host.on_fetch(move |_| {
				let chunk = bytes::Bytes::from(format!(
					"data: {}\n\n",
					json!({ "id": "c", "choices": [{ "delta": { "content": "hi" } }] })
				));
				let mut response = HttpResponse::from_bytes(200, IndexMap::new(), "");
				response.body = futures::stream::once(async move { Ok(chunk) })
					.chain(futures::stream::pending())
					.boxed_local();
				Box::pin(async move { Ok(response) })
			});
			let mut request = options("k");
			request.signal = Some(signal);
			let model = model("m", "custom", "https://example.test/v1");
			let events = stream(&model, &context(vec![user_message("hi")]), Some(request));
			pi_host::spawn_local(async move {
				pi_host::sleep(20.0, None).await.ok();
				controller.abort();
			});
			let result = events.result().await.unwrap();
			assert_eq!(result.stop_reason, StopReason::Aborted);
			assert_eq!(result.error_message.as_deref(), Some("Request was aborted"));
			assert!(matches!(&result.content[..], [AssistantContent::Text(text)] if text.text == "hi"));
		});
	}
}

mod non_stream_payloads {
	use super::*;
	use std::rc::Rc;

	fn replace_payload(value: serde_json::Value) -> pi_ai::types::OnPayload {
		Rc::new(move |_, _| {
			let value = value.clone();
			Box::pin(async move { Ok(Some(value)) })
		})
	}

	#[test]
	fn fails_before_sending_when_on_payload_returns_null() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async {
					Ok(HttpResponse::from_bytes(
						200,
						IndexMap::new(),
						sse_body(&[text_chunk()]),
					))
				})
			});
			let mut request = options("k");
			request.on_payload = Some(replace_payload(serde_json::Value::Null));
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(request))
				.result()
				.await
				.unwrap();
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Cannot read properties of null (reading 'stream')")
			);
			assert!(host.requests().is_empty());
		});
	}

	#[test]
	fn iterates_a_text_body_like_for_await_over_a_string() {
		run(|host| async move {
			host.on_fetch(|_| {
				Box::pin(async {
					Ok(HttpResponse::from_bytes(
						200,
						IndexMap::new(),
						sse_body(&[text_chunk()]),
					))
				})
			});
			let mut request = options("k");
			request.on_payload = Some(replace_payload(json!(5)));
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(request))
				.result()
				.await
				.unwrap();
			assert_eq!(
				host.requests().pop().and_then(|request| request.body).as_deref(),
				Some("5")
			);
			assert_eq!(result.stop_reason, StopReason::Error);
			assert_eq!(
				result.error_message.as_deref(),
				Some("Stream ended without finish_reason")
			);
		});
	}

	#[test]
	fn iterates_a_json_array_body_as_chunks() {
		run(|host| async move {
			host.on_fetch(|_| {
				let body = pi_host::js::json_stringify(&json!([text_chunk()]));
				let headers = IndexMap::from([("content-type".to_string(), "application/json".to_string())]);
				Box::pin(async move { Ok(HttpResponse::from_bytes(200, headers, body)) })
			});
			let mut request = options("k");
			request.on_payload = Some(replace_payload(
				json!({ "model": "m", "messages": [], "stream": false }),
			));
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(request))
				.result()
				.await
				.unwrap();
			assert_eq!(result.error_message, None);
			assert_eq!(result.stop_reason, StopReason::Stop);
			assert!(matches!(&result.content[..], [AssistantContent::Text(text)] if text.text == "hi"));
		});
	}

	#[test]
	fn fails_on_a_json_object_body() {
		run(|host| async move {
			host.on_fetch(|_| {
				let headers = IndexMap::from([("content-type".to_string(), "application/json".to_string())]);
				Box::pin(async move { Ok(HttpResponse::from_bytes(200, headers, r#"{"id":"c"}"#)) })
			});
			let mut request = options("k");
			request.on_payload = Some(replace_payload(json!({ "model": "m", "messages": [] })));
			let model = model("m", "custom", "https://example.test/v1");
			let result = stream(&model, &context(vec![user_message("hi")]), Some(request))
				.result()
				.await
				.unwrap();
			assert_eq!(
				result.error_message.as_deref(),
				Some("openaiStream is not async iterable")
			);
		});
	}
}

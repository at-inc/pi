//! Port of `packages/mcp/test/streamable-http.test.ts`.
//!
//! The `node:http` loopback servers are the in-process `FakeNetwork` of `tests/common`. "calls fetch without a
//! receiver" (#10188) has no receiver to check in Rust; it still checks that a custom `fetch` and the host's
//! `fetch` both serve the transport and the auth provider's `context.fetch`.

mod common;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bytes::Bytes;
use common::{Gate, ServerRequest, ServerResponse, VirtualHost, norm, run, spawn};
use futures::{FutureExt, StreamExt};
use pi_host::{AbortController, HttpRequest, LocalBoxFuture, PiResult};
use pi_mcp::auth_provider::{AuthProvider, McpFetch};
use pi_mcp::client::{McpClient, McpClientOptions, McpRequestOptions};
use pi_mcp::protocol::types::LATEST_PROTOCOL_VERSION;
use pi_mcp::transports::streamable_http::{
	ConsumeSseOptions, McpAuthRequiredError, McpHttpError, McpSessionExpiredError, SseEvent,
	StreamableHttpReconnectOptions, StreamableHttpTransport, StreamableHttpTransportOptions, consume_sse_stream,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct RecordedRequest {
	method: String,
	request: ServerRequest,
	message: Option<Value>,
}

type Requests = Rc<RefCell<Vec<RecordedRequest>>>;

fn start_server(
	host: &VirtualHost,
	handler: impl Fn(ServerRequest, ServerResponse, Requests) -> LocalBoxFuture<'static, PiResult<()>> + 'static,
) -> (String, Requests) {
	let requests: Requests = Rc::new(RefCell::new(Vec::new()));
	let recorded = requests.clone();
	let origin = host.listen(move |request, response| handler(request, response, recorded.clone()));
	(format!("{origin}/mcp"), requests)
}

fn protocol(
	request: ServerRequest,
	response: ServerResponse,
	requests: Requests,
) -> LocalBoxFuture<'static, PiResult<()>> {
	Box::pin(async move { protocol_handler(request, response, requests, None).await })
}

async fn protocol_handler(
	request: ServerRequest,
	response: ServerResponse,
	requests: Requests,
	body: Option<Value>,
) -> PiResult<()> {
	if request.method == "GET" || request.method == "DELETE" {
		let method = request.method.clone();
		response.set_status(if method == "GET" { 405 } else { 200 });
		requests.borrow_mut().push(RecordedRequest {
			method,
			request,
			message: None,
		});
		response.end("");
		return Ok(());
	}
	let message = body.unwrap_or_else(|| request.json());
	requests.borrow_mut().push(RecordedRequest {
		method: request.method.clone(),
		request,
		message: Some(message.clone()),
	});
	let Some(id) = message.get("id") else {
		response.set_status(202);
		response.end("");
		return Ok(());
	};
	let method = message.get("method").and_then(Value::as_str);
	if method == Some("initialize") {
		response.write_head(
			200,
			&[("content-type", "application/json"), ("mcp-session-id", "session-1")],
		);
		response.end(
			&json!({
				"jsonrpc": "2.0",
				"id": id,
				"result": {
					"protocolVersion": LATEST_PROTOCOL_VERSION,
					"capabilities": { "tools": {} },
					"serverInfo": { "name": "http-fixture", "version": "1.0.0" },
				},
			})
			.to_string(),
		);
		return Ok(());
	}
	if method == Some("tools/list") {
		response.set_header("content-type", "application/json");
		response.end(
			&json!({
				"jsonrpc": "2.0",
				"id": id,
				"result": { "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] },
			})
			.to_string(),
		);
		return Ok(());
	}
	response.write_head(200, &[("content-type", "text/event-stream")]);
	response.write("id: tool-result\n");
	let result = json!({ "jsonrpc": "2.0", "id": id, "result": { "content": [{ "type": "text", "text": "hello" }] } });
	response.end(&format!("data: {result}\n\n"));
	Ok(())
}

fn client() -> McpClient {
	McpClient::new(McpClientOptions {
		name: "http-test".into(),
		version: "1.0.0".into(),
		..Default::default()
	})
}

fn transport(options: StreamableHttpTransportOptions) -> StreamableHttpTransport {
	StreamableHttpTransport::new(options).expect("valid URL")
}

fn http(url: &str) -> StreamableHttpTransportOptions {
	StreamableHttpTransportOptions {
		url: url.to_string(),
		..Default::default()
	}
}

fn without_get_stream(url: &str) -> StreamableHttpTransportOptions {
	StreamableHttpTransportOptions {
		open_get_stream: Some(false),
		..http(url)
	}
}

fn options() -> McpRequestOptions {
	McpRequestOptions::default()
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
	norm(&serde_json::to_value(value).unwrap())
}

fn hello() -> Value {
	json!({ "content": [{ "type": "text", "text": "hello" }] })
}

fn echo_tools() -> Value {
	json!([{ "name": "echo", "inputSchema": { "type": "object" } }])
}

fn tool_name(message: &Value) -> Option<String> {
	message.get("params")?.get("name")?.as_str().map(str::to_string)
}

fn is_tools_call(message: &Value) -> bool {
	message.get("method").and_then(Value::as_str) == Some("tools/call")
}

mod consume_sse_stream_tests {
	use super::*;

	#[test]
	fn parses_chunked_crlf_events_comments_ids_and_multiline_data() {
		run(|_| async {
			let chunks = [": keepalive\r\nid: 7\r\ndata: {\"one\":\r\n", "data: 1}\r\n\r\n"];
			let stream = futures::stream::iter(chunks.map(|chunk| Ok(Bytes::from(chunk)))).boxed_local();
			let events: RefCell<Vec<SseEvent>> = RefCell::new(Vec::new());
			consume_sse_stream(
				stream,
				ConsumeSseOptions {
					on_event: Some(Box::new(|event| events.borrow_mut().push(event))),
					..Default::default()
				},
			)
			.await
			.unwrap();
			assert_eq!(
				events.into_inner(),
				vec![SseEvent {
					event: None,
					id: Some("7".into()),
					data: "{\"one\":\n1}".into(),
				}]
			);
		});
	}

	#[test]
	fn rejects_events_whose_data_lines_exceed_the_limit_without_a_blank_line() {
		run(|_| async {
			let sent = Rc::new(Cell::new(0));
			let counter = sent.clone();
			// Never sends a blank line, so the event is never dispatched.
			let stream = futures::stream::unfold((), move |()| {
				let counter = counter.clone();
				async move {
					let count = counter.get();
					counter.set(count + 1);
					(count <= 1000).then(|| (Ok(Bytes::from_static(b"data: xxxxxxxxxxxxxxxx\n")), ()))
				}
			})
			.boxed_local();
			let error = consume_sse_stream(
				stream,
				ConsumeSseOptions {
					max_event_bytes: Some(256),
					on_event: Some(Box::new(|_| {})),
					..Default::default()
				},
			)
			.await
			.unwrap_err();
			assert!(
				error.message.contains("MCP SSE event exceeds 256 bytes"),
				"{}",
				error.message
			);
			assert!(sent.get() < 100, "{}", sent.get());
		});
	}
}

mod streamable_http_transport {
	use super::*;

	#[test]
	fn handles_json_and_sse_responses_with_session_and_protocol_headers() {
		run(|host| async move {
			let (url, requests) = start_server(&host, protocol);
			let transport = transport(http(&url));
			let client = client();
			client.connect(Rc::new(transport.clone())).await.unwrap();
			assert_eq!(transport.session_id().as_deref(), Some("session-1"));
			assert_eq!(to_json(&client.list_tools(options()).await.unwrap()), echo_tools());
			let args = json!({ "text": "hello" }).as_object().cloned();
			assert_eq!(
				to_json(&client.call_tool("echo", args, options()).await.unwrap()),
				hello()
			);
			client.close().await.unwrap();

			let requests = requests.borrow();
			let list_request = requests
				.iter()
				.find(|entry| {
					entry.message.as_ref().and_then(|message| message.get("method")) == Some(&json!("tools/list"))
				})
				.expect("tools/list request");
			assert_eq!(list_request.request.header("mcp-session-id"), Some("session-1"));
			assert_eq!(
				list_request.request.header("mcp-protocol-version"),
				Some(LATEST_PROTOCOL_VERSION)
			);
			assert!(requests.iter().any(|entry| entry.method == "GET"));
			assert!(requests.iter().any(|entry| entry.method == "DELETE"));
		});
	}

	#[test]
	fn classifies_authentication_failures() {
		run(|host| async move {
			let (url, _) = start_server(&host, |_, response, _| {
				Box::pin(async move {
					response.write_head(
						401,
						&[(
							"www-authenticate",
							"Bearer resource_metadata=\"https://example.com/meta\"",
						)],
					);
					response.end("login required");
					Ok(())
				})
			});
			let error = client().connect(Rc::new(transport(http(&url)))).await.unwrap_err();
			assert_eq!(error.name, "McpAuthRequiredError");
			assert!(McpAuthRequiredError::is(&error) && McpHttpError::is(&error));
			assert_eq!(McpHttpError::status(&error), Some(401));
			assert_eq!(McpHttpError::body(&error).as_deref(), Some("login required"));
			assert_eq!(
				McpAuthRequiredError::www_authenticate(&error).as_deref(),
				Some("Bearer resource_metadata=\"https://example.com/meta\"")
			);
		});
	}

	#[test]
	fn fails_only_the_request_whose_sse_stream_breaks() {
		run(|host| async move {
			let slow_gate = Gate::new();
			let gate = slow_gate.clone();
			let (url, _) = start_server(&host, move |request, response, requests| {
				let gate = gate.clone();
				Box::pin(async move {
					if request.method != "POST" {
						return protocol_handler(request, response, requests, None).await;
					}
					let message = request.json();
					let name = tool_name(&message);
					if name.as_deref() == Some("broken") {
						response.write_head(200, &[("content-type", "text/event-stream")]);
						response.end("data: not json\n\n");
						return Ok(());
					}
					if name.as_deref() == Some("slow") {
						gate.wait().await;
					}
					protocol_handler(request, response, requests, Some(message)).await
				})
			});
			let client = client();
			let errors = Rc::new(RefCell::new(Vec::new()));
			let seen = errors.clone();
			let _ = client.on_error(Rc::new(move |error| seen.borrow_mut().push(error.clone())));
			client
				.connect(Rc::new(transport(without_get_stream(&url))))
				.await
				.unwrap();

			let slow = spawn(client.call_tool("slow", None, options()));
			let error = client.call_tool("broken", None, options()).await.unwrap_err();
			assert!(
				error.message.contains("MCP response stream failed"),
				"{}",
				error.message
			);
			slow_gate.release();
			assert_eq!(to_json(&slow.await.unwrap()), hello());
			assert_eq!(errors.borrow().len(), 1);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn opens_the_get_stream_after_initialization_and_sends_last_event_id_only_when_resuming() {
		run(|host| async move {
			let order = Rc::new(RefCell::new(Vec::<String>::new()));
			let seen = order.clone();
			let (url, requests) = start_server(&host, move |request, response, requests| {
				let seen = seen.clone();
				Box::pin(async move {
					if request.method == "POST" {
						let message = request.json();
						let method = message.get("method").and_then(Value::as_str).unwrap_or("undefined");
						seen.borrow_mut().push(method.to_string());
						return protocol_handler(request, response, requests, Some(message)).await;
					}
					seen.borrow_mut().push(request.method.clone());
					protocol_handler(request, response, requests, None).await
				})
			});
			let client = client();
			client.connect(Rc::new(transport(http(&url)))).await.unwrap();
			client.call_tool("echo", None, options()).await.unwrap();
			client.list_tools(options()).await.unwrap();
			client.close().await.unwrap();
			let order = order.borrow();
			let index_of = |name: &str| {
				order
					.iter()
					.position(|entry| entry == name)
					.map_or(-1, |index| index as i64)
			};
			assert!(index_of("GET") > index_of("notifications/initialized"), "{order:?}");
			assert!(
				requests
					.borrow()
					.iter()
					.all(|entry| entry.request.header("last-event-id").is_none())
			);
		});
	}

	#[test]
	fn resumes_a_response_stream_the_server_closed_before_answering() {
		run(|host| async move {
			let resume_headers = Rc::new(RefCell::new(Vec::<String>::new()));
			let seen = resume_headers.clone();
			let (url, _) = start_server(&host, move |request, response, requests| {
				let seen = seen.clone();
				Box::pin(async move {
					if request.method == "GET"
						&& let Some(last_event_id) = request.header("last-event-id").filter(|id| !id.is_empty())
					{
						seen.borrow_mut().push(last_event_id.to_string());
						response.write_head(200, &[("content-type", "text/event-stream")]);
						let result = json!({ "jsonrpc": "2.0", "id": 2, "result": { "content": [{ "type": "text", "text": "resumed" }] } });
						response.end(&format!("id: 2\ndata: {result}\n\n"));
						return Ok(());
					}
					if request.method != "POST" {
						return protocol_handler(request, response, requests, None).await;
					}
					let message = request.json();
					if !is_tools_call(&message) {
						return protocol_handler(request, response, requests, Some(message)).await;
					}
					// Priming event (ID, no data) and a retry hint, then the server drops the stream.
					response.write_head(200, &[("content-type", "text/event-stream")]);
					response.end("id: 1\nretry: 5\ndata:\n\n");
					Ok(())
				})
			});
			let client = client();
			let errors = Rc::new(RefCell::new(Vec::<String>::new()));
			let seen_errors = errors.clone();
			let _ = client.on_error(Rc::new(move |error| {
				seen_errors.borrow_mut().push(error.message.clone())
			}));
			client
				.connect(Rc::new(transport(without_get_stream(&url))))
				.await
				.unwrap();
			assert_eq!(
				to_json(&client.call_tool("echo", None, options()).await.unwrap()),
				json!({ "content": [{ "type": "text", "text": "resumed" }] })
			);
			assert_eq!(*resume_headers.borrow(), vec!["1".to_string()]);
			assert!(errors.borrow().is_empty(), "{:?}", errors.borrow());
			client.close().await.unwrap();
		});
	}

	#[test]
	fn fails_a_request_whose_response_stream_ends_without_an_answer() {
		run(|host| async move {
			let (url, _) = start_server(&host, |request, response, requests| {
				Box::pin(async move {
					if request.method != "POST" {
						return protocol_handler(request, response, requests, None).await;
					}
					let message = request.json();
					if !is_tools_call(&message) {
						return protocol_handler(request, response, requests, Some(message)).await;
					}
					response.write_head(200, &[("content-type", "text/event-stream")]);
					response.end(": nothing here\n\n");
					Ok(())
				})
			});
			let client = client();
			client
				.connect(Rc::new(transport(without_get_stream(&url))))
				.await
				.unwrap();
			let error = client
				.call_tool(
					"echo",
					Some(Default::default()),
					McpRequestOptions {
						timeout_ms: Some(5_000.0),
						..options()
					},
				)
				.await
				.unwrap_err();
			assert!(
				error
					.message
					.contains("MCP response stream failed: stream ended without a response"),
				"{}",
				error.message
			);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn reconnects_the_get_stream_after_it_drops() {
		run(|host| async move {
			let gets = Rc::new(Cell::new(0));
			let last_event_ids = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
			let (counter, seen) = (gets.clone(), last_event_ids.clone());
			let (url, _) = start_server(&host, move |request, response, requests| {
				let (counter, seen) = (counter.clone(), seen.clone());
				Box::pin(async move {
					if request.method != "GET" {
						return protocol_handler(request, response, requests, None).await;
					}
					counter.set(counter.get() + 1);
					seen.borrow_mut()
						.push(request.header("last-event-id").map(str::to_string));
					response.write_head(200, &[("content-type", "text/event-stream")]);
					let notification = json!({ "jsonrpc": "2.0", "method": "notifications/tools/list_changed" });
					if counter.get() == 1 {
						response.end(&format!("id: g1\ndata: {notification}\n\n"));
						return Ok(());
					}
					response.write(&format!("id: g2\ndata: {notification}\n\n"));
					Ok(())
				})
			});
			let client = client();
			let changes = Rc::new(Cell::new(0));
			let second_change = Gate::new();
			let gate = second_change.clone();
			let _ = client.on_notification(
				"notifications/tools/list_changed",
				Rc::new(move |_| {
					changes.set(changes.get() + 1);
					if changes.get() == 2 {
						gate.release();
					}
					Ok(())
				}),
			);
			client
				.connect(Rc::new(transport(StreamableHttpTransportOptions {
					reconnect: Some(StreamableHttpReconnectOptions {
						initial_delay_ms: Some(1.0),
						..Default::default()
					}),
					..http(&url)
				})))
				.await
				.unwrap();
			second_change.wait().await;
			assert_eq!(*last_event_ids.borrow(), vec![None, Some("g1".to_string())]);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn rejects_a_request_the_server_accepts_without_a_response() {
		run(|host| async move {
			let (url, _) = start_server(&host, |request, response, requests| {
				Box::pin(async move {
					if request.method != "POST" {
						return protocol_handler(request, response, requests, None).await;
					}
					let message = request.json();
					if !is_tools_call(&message) {
						return protocol_handler(request, response, requests, Some(message)).await;
					}
					response.set_status(202);
					response.end("");
					Ok(())
				})
			});
			let client = client();
			client
				.connect(Rc::new(transport(without_get_stream(&url))))
				.await
				.unwrap();
			let error = client.call_tool("echo", None, options()).await.unwrap_err();
			assert!(error.message.contains("without a response"), "{}", error.message);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn includes_the_response_body_in_http_errors() {
		run(|host| async move {
			let (url, _) = start_server(&host, |_, response, _| {
				Box::pin(async move {
					response.set_status(400);
					response.end("Invalid Accept header");
					Ok(())
				})
			});
			let error = client().connect(Rc::new(transport(http(&url)))).await.unwrap_err();
			assert!(
				error
					.message
					.contains("MCP HTTP request failed with status 400: Invalid Accept header"),
				"{}",
				error.message
			);
		});
	}

	#[test]
	fn hands_401_and_insufficient_scope_403_responses_to_the_auth_provider_with_the_rejected_token() {
		run(|host| async move {
			let seen = Rc::new(RefCell::new(Vec::<(u16, Option<String>)>::new()));
			let token = Rc::new(RefCell::new("old".to_string()));
			let (url, _) = start_server(&host, |request, response, requests| {
				Box::pin(async move {
					if request.method != "POST" {
						return protocol_handler(request, response, requests, None).await;
					}
					let message = request.json();
					if request.header("authorization") == Some("Bearer old") {
						response.write_head(401, &[("www-authenticate", "Bearer")]).end("");
						return Ok(());
					}
					if is_tools_call(&message) && request.header("authorization") == Some("Bearer new") {
						response
							.write_head(
								403,
								&[(
									"www-authenticate",
									"Bearer error=\"insufficient_scope\", scope=\"admin\"",
								)],
							)
							.end("");
						return Ok(());
					}
					protocol_handler(request, response, requests, Some(message)).await
				})
			});
			let (current, next, record) = (token.clone(), token.clone(), seen.clone());
			let client = client();
			client
				.connect(Rc::new(transport(StreamableHttpTransportOptions {
					auth_provider: Some(AuthProvider {
						token: Rc::new(move || {
							let token = current.borrow().clone();
							Box::pin(async move { Ok(Some(token)) })
						}),
						on_unauthorized: Some(Rc::new(move |context| {
							record
								.borrow_mut()
								.push((context.response.status, context.token.clone()));
							*next.borrow_mut() =
								if context.response.status == 401 { "new" } else { "admin" }.to_string();
							Box::pin(async { Ok(()) })
						})),
					}),
					..without_get_stream(&url)
				})))
				.await
				.unwrap();
			assert_eq!(
				to_json(&client.call_tool("echo", None, options()).await.unwrap()),
				hello()
			);
			assert_eq!(
				*seen.borrow(),
				vec![(401, Some("old".to_string())), (403, Some("new".to_string()))]
			);
			client.close().await.unwrap();
		});
	}

	// #10598: a request cancelled or timed out while waiting for a shared refresh must not be retried.
	#[test]
	fn does_not_retry_requests_that_were_cancelled_while_authorization_was_recovering() {
		run(|host| async move {
			let token = Rc::new(RefCell::new("old".to_string()));
			let executed = Rc::new(RefCell::new(Vec::<String>::new()));
			let ran = executed.clone();
			let (url, _) = start_server(&host, move |request, response, requests| {
				let ran = ran.clone();
				Box::pin(async move {
					if request.method != "POST" {
						return protocol_handler(request, response, requests, None).await;
					}
					let message = request.json();
					if is_tools_call(&message) && request.header("authorization") == Some("Bearer old") {
						response.write_head(401, &[("www-authenticate", "Bearer")]).end("");
						return Ok(());
					}
					if is_tools_call(&message) {
						ran.borrow_mut().push(tool_name(&message).unwrap_or_default());
					}
					protocol_handler(request, response, requests, Some(message)).await
				})
			});
			let refresh_gate = Gate::new();
			let refresh: Rc<RefCell<Option<futures::future::Shared<LocalBoxFuture<'static, ()>>>>> =
				Rc::new(RefCell::new(None));
			let refreshes = Rc::new(Cell::new(0));
			let waiting = Rc::new(Cell::new(0));
			let waiting_gate = Gate::new();
			let current = token.clone();
			let (gate, all_waiting, count, waiters) = (
				refresh_gate.clone(),
				waiting_gate.clone(),
				refreshes.clone(),
				waiting.clone(),
			);
			let on_unauthorized = Rc::new(move |_| -> LocalBoxFuture<'static, PiResult<()>> {
				let shared = refresh
					.borrow_mut()
					.get_or_insert_with(|| {
						count.set(count.get() + 1);
						let (gate, token) = (gate.clone(), token.clone());
						let refresh: LocalBoxFuture<'static, ()> = Box::pin(async move {
							gate.wait().await;
							*token.borrow_mut() = "new".into();
						});
						refresh.shared()
					})
					.clone();
				waiters.set(waiters.get() + 1);
				if waiters.get() == 3 {
					all_waiting.release();
				}
				Box::pin(async move {
					shared.await;
					Ok(())
				})
			});
			let client = client();
			client
				.connect(Rc::new(transport(StreamableHttpTransportOptions {
					auth_provider: Some(AuthProvider {
						token: Rc::new(move || {
							let token = current.borrow().clone();
							Box::pin(async move { Ok(Some(token)) })
						}),
						on_unauthorized: Some(on_unauthorized),
					}),
					..without_get_stream(&url)
				})))
				.await
				.unwrap();
			let controller = AbortController::new();
			let empty = || Some(Default::default());
			let aborted = spawn(client.call_tool(
				"aborted",
				empty(),
				McpRequestOptions {
					signal: Some(controller.signal()),
					..options()
				},
			));
			let timed_out = spawn(client.call_tool(
				"timed-out",
				empty(),
				McpRequestOptions {
					timeout_ms: Some(200.0),
					..options()
				},
			));
			let kept = spawn(client.call_tool("kept", None, options()));
			waiting_gate.wait().await;
			controller.abort();
			assert_eq!(aborted.await.unwrap_err().name, "AbortError");
			assert_eq!(timed_out.await.unwrap_err().name, "McpTimeoutError");
			refresh_gate.release();
			assert_eq!(to_json(&kept.await.unwrap()), hello());
			assert_eq!(*executed.borrow(), vec!["kept".to_string()]);
			assert_eq!(refreshes.get(), 1);
			client.close().await.unwrap();
		});
	}

	// #10565: the auth provider may refresh over the network, which closing must not wait for.
	#[test]
	fn closes_the_session_with_the_last_requests_token_without_asking_the_auth_provider() {
		run(|host| async move {
			let (url, requests) = start_server(&host, protocol);
			let calls = Rc::new(Cell::new(0));
			let counter = calls.clone();
			let client = client();
			client
				.connect(Rc::new(transport(StreamableHttpTransportOptions {
					auth_provider: Some(AuthProvider {
						token: Rc::new(move || {
							counter.set(counter.get() + 1);
							let token = format!("token-{}", counter.get());
							Box::pin(async move { Ok(Some(token)) })
						}),
						on_unauthorized: None,
					}),
					..without_get_stream(&url)
				})))
				.await
				.unwrap();
			let before = calls.get();
			client.close().await.unwrap();
			assert_eq!(calls.get(), before);
			let requests = requests.borrow();
			let deletes: Vec<&RecordedRequest> = requests.iter().filter(|entry| entry.method == "DELETE").collect();
			assert_eq!(
				deletes
					.iter()
					.map(|entry| entry.request.header("authorization"))
					.collect::<Vec<_>>(),
				vec![Some(format!("Bearer token-{before}").as_str())]
			);
			assert_eq!(deletes[0].request.header("mcp-session-id"), Some("session-1"));
		});
	}

	// #10188: Cloudflare Workers reject the platform fetch when called with a receiver other than globalThis.
	#[test]
	fn calls_fetch_without_a_receiver() {
		run(|host| async move {
			let custom_calls = Rc::new(Cell::new(0));
			let counter = custom_calls.clone();
			let custom_fetch: McpFetch = Rc::new(move |request: HttpRequest| {
				counter.set(counter.get() + 1);
				Box::pin(async move { pi_host::fetch(request, None).await })
			});
			let (url, _) = start_server(&host, |request, response, requests| {
				Box::pin(async move {
					if request.method == "POST" && request.header("authorization").is_none() {
						response.write_head(401, &[("www-authenticate", "Bearer")]).end("");
						return Ok(());
					}
					protocol_handler(request, response, requests, None).await
				})
			});
			let connect = |fetch: Option<McpFetch>| {
				let url = url.clone();
				async move {
					let token: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
					let (current, next) = (token.clone(), token.clone());
					let client = client();
					let probe_url = url.clone();
					client
						.connect(Rc::new(transport(StreamableHttpTransportOptions {
							fetch,
							auth_provider: Some(AuthProvider {
								token: Rc::new(move || {
									let token = current.borrow().clone();
									Box::pin(async move { Ok(token) })
								}),
								on_unauthorized: Some(Rc::new(move |context| {
									let (next, url) = (next.clone(), probe_url.clone());
									Box::pin(async move {
										drop(
											(context.fetch)(HttpRequest {
												method: "GET".into(),
												url,
												headers: Default::default(),
												body: None,
												signal: None,
												..Default::default()
											})
											.await?,
										);
										*next.borrow_mut() = Some("token".into());
										Ok(())
									})
								})),
							}),
							..without_get_stream(&url)
						})))
						.await
						.unwrap();
					assert_eq!(to_json(&client.list_tools(options()).await.unwrap()), echo_tools());
					client.close().await.unwrap();
				}
			};

			connect(Some(custom_fetch)).await;
			assert!(custom_calls.get() > 0);
			connect(None).await;
		});
	}

	#[test]
	fn classifies_an_expired_established_session() {
		run(|host| async move {
			let posts = Rc::new(Cell::new(0));
			let (url, _) = start_server(&host, move |request, response, requests| {
				let posts = posts.clone();
				Box::pin(async move {
					if request.method == "POST" {
						let count = posts.get();
						posts.set(count + 1);
						if count >= 2 {
							response.set_status(404);
							response.end("gone");
							return Ok(());
						}
					}
					protocol_handler(request, response, requests, None).await
				})
			});
			let client = client();
			client
				.connect(Rc::new(transport(without_get_stream(&url))))
				.await
				.unwrap();
			let error = client.list_tools(options()).await.unwrap_err();
			assert!(McpSessionExpiredError::is(&error), "{}: {}", error.name, error.message);
			client.close().await.unwrap();
		});
	}
}

//! Port of `packages/mcp/test/client.test.ts`.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use common::{Gate, json, norm, run, sleep};
use pi_host::{AbortController, LocalBoxFuture, PiError, PiResult};
use pi_mcp::client::{McpClient, McpClientOptions, McpRequestOptions, McpRoots};
use pi_mcp::protocol::jsonrpc::{
	JsonRpcErrorObject, JsonRpcMessage, JsonRpcRequest, McpAbortError, McpError, McpTimeoutError,
};
use pi_mcp::protocol::types::{LATEST_PROTOCOL_VERSION, Root};
use pi_mcp::testing::{InMemoryTransport, create_in_memory_transport_pair};
use pi_mcp::transports::transport::{McpTransport, McpTransportSendOptions};
use serde_json::{Map, Value, json};

type Handler = Rc<dyn Fn(JsonRpcRequest) -> LocalBoxFuture<'static, PiResult<Value>>>;

#[derive(Clone)]
struct TestServer {
	transport: InMemoryTransport,
	messages: Rc<RefCell<Vec<Value>>>,
	handlers: Rc<RefCell<HashMap<String, Handler>>>,
}

impl TestServer {
	fn set_handler(
		&self,
		method: &str,
		handler: impl Fn(JsonRpcRequest) -> LocalBoxFuture<'static, PiResult<Value>> + 'static,
	) {
		self.handlers.borrow_mut().insert(method.to_string(), Rc::new(handler));
	}

	fn set_result(&self, method: &str, result: Value) {
		self.set_handler(method, move |_| {
			let result = result.clone();
			Box::pin(async move { Ok(result) })
		});
	}

	fn messages(&self) -> Vec<Value> {
		self.messages.borrow().clone()
	}
}

fn send(transport: &InMemoryTransport, message: JsonRpcMessage) -> LocalBoxFuture<'static, PiResult<()>> {
	transport.send(message, McpTransportSendOptions::default())
}

async fn create_server() -> (InMemoryTransport, TestServer) {
	let pair = create_in_memory_transport_pair();
	let server = TestServer {
		transport: pair.server.clone(),
		messages: Rc::new(RefCell::new(Vec::new())),
		handlers: Rc::new(RefCell::new(HashMap::new())),
	};
	let listener_server = server.clone();
	let _ = pair.server.on_message(Rc::new(move |message| {
		listener_server.messages.borrow_mut().push(norm(&message.to_value()));
		let JsonRpcMessage::Request(request) = message.clone() else {
			return;
		};
		let handler = listener_server.handlers.borrow().get(&request.method).cloned();
		let transport = listener_server.transport.clone();
		pi_host::spawn_local(async move {
			let result = match handler {
				None => Err(McpError::new(
					-32601.0,
					format!("Method not found: {}", request.method),
					None,
				)),
				Some(handler) => handler(request.clone()).await,
			};
			let message = match result {
				Ok(result) => JsonRpcMessage::result(request.id.clone(), result),
				Err(error) => {
					let error = if McpError::is(&error) {
						error
					} else {
						McpError::new(-32603.0, pi_mcp::protocol::jsonrpc::js_error_string(&error), None)
					};
					JsonRpcMessage::error(
						request.id.clone(),
						JsonRpcErrorObject {
							code: McpError::code(&error).expect("McpError has a code"),
							message: error.message.clone(),
							data: McpError::data(&error),
						},
					)
				}
			};
			let _ = send(&transport, message).await;
		});
	}));
	pair.server.start().await.unwrap();
	server.set_result(
		"initialize",
		json!({
			"protocolVersion": LATEST_PROTOCOL_VERSION,
			"capabilities": { "tools": { "listChanged": true } },
			"serverInfo": { "name": "test-server", "version": "1.0.0" },
			"instructions": "Use test tools.",
		}),
	);
	(pair.client, server)
}

fn client_options(version: &str) -> McpClientOptions {
	McpClientOptions {
		name: "test-client".into(),
		version: version.into(),
		..Default::default()
	}
}

async fn connect() -> (McpClient, TestServer) {
	let (client_transport, server) = create_server().await;
	let client = McpClient::new(client_options("2.0.0"));
	client.connect(Rc::new(client_transport)).await.unwrap();
	(client, server)
}

fn options() -> McpRequestOptions {
	McpRequestOptions::default()
}

fn args(value: Value) -> Option<Map<String, Value>> {
	match value {
		Value::Object(map) => Some(map),
		_ => None,
	}
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
	norm(&serde_json::to_value(value).unwrap())
}

fn param<'a>(request: &'a JsonRpcRequest, key: &str) -> Option<&'a Value> {
	request.params.as_ref().and_then(|params| params.get(key))
}

mod mcp_client {
	use super::*;

	#[test]
	fn initializes_the_connection_before_exposing_server_information() {
		run(|_| async {
			let (client, server) = connect().await;
			assert_eq!(client.connection_state().as_str(), "connected");
			assert_eq!(client.protocol_version().as_deref(), Some(LATEST_PROTOCOL_VERSION));
			assert_eq!(
				to_json(&client.server_info()),
				json(r#"{"name":"test-server","version":"1.0.0"}"#)
			);
			assert_eq!(
				to_json(&client.server_capabilities()),
				json(r#"{"tools":{"listChanged":true}}"#)
			);
			assert_eq!(client.instructions().as_deref(), Some("Use test tools."));
			assert_eq!(
				server.messages(),
				vec![
					norm(&json!({
						"jsonrpc": "2.0",
						"id": 1,
						"method": "initialize",
						"params": {
							"protocolVersion": LATEST_PROTOCOL_VERSION,
							"capabilities": {},
							"clientInfo": { "name": "test-client", "version": "2.0.0" },
						},
					})),
					json(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
				]
			);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn paginates_tools_and_preserves_protocol_tool_definitions() {
		run(|_| async {
			let (client, server) = connect().await;
			server.set_handler("tools/list", |request| {
				let cursor = param(&request, "cursor").cloned();
				Box::pin(async move {
					Ok(match cursor {
						None => json!({
							"tools": [{ "name": "search", "description": "Search", "inputSchema": { "type": "object" } }],
							"nextCursor": "page-2",
						}),
						Some(_) => json!({
							"tools": [{
								"name": "read",
								"inputSchema": { "type": "object" },
								"outputSchema": { "type": "object" },
								"annotations": { "readOnlyHint": true },
							}],
							// Some servers end pagination with an empty cursor instead of omitting it.
							"nextCursor": "",
						}),
					})
				})
			});
			assert_eq!(
				to_json(&client.list_tools(options()).await.unwrap()),
				json(
					r#"[
						{ "name": "search", "description": "Search", "inputSchema": { "type": "object" } },
						{
							"name": "read",
							"inputSchema": { "type": "object" },
							"outputSchema": { "type": "object" },
							"annotations": { "readOnlyHint": true }
						}
					]"#
				)
			);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn lists_and_reads_resources() {
		run(|_| async {
			let (client, server) = connect().await;
			server.set_handler("resources/list", |request| {
				let cursor = param(&request, "cursor").cloned();
				Box::pin(async move {
					Ok(match cursor {
						None => {
							json!({ "resources": [{ "uri": "file:///a", "name": "a", "mimeType": "text/plain" }], "nextCursor": "2" })
						}
						Some(_) => json!({ "resources": [{ "uri": "file:///b" }] }),
					})
				})
			});
			server.set_result(
				"resources/templates/list",
				json!({ "resourceTemplates": [{ "uriTemplate": "repo://{owner}/{repo}", "name": "repo" }] }),
			);
			server.set_handler("resources/read", |request| {
				let uri = param(&request, "uri").cloned().unwrap();
				Box::pin(async move { Ok(json!({ "contents": [{ "uri": uri, "text": "hello" }] })) })
			});
			// A missing name falls back to the URI.
			assert_eq!(
				to_json(&client.list_resources(options()).await.unwrap()),
				json(
					r#"[{ "uri": "file:///a", "name": "a", "mimeType": "text/plain" }, { "uri": "file:///b", "name": "file:///b" }]"#
				)
			);
			assert_eq!(
				to_json(&client.list_resource_templates(options()).await.unwrap()),
				json(r#"[{ "uriTemplate": "repo://{owner}/{repo}", "name": "repo" }]"#)
			);
			// Single pages pass the cursor through.
			assert_eq!(
				to_json(&client.list_resources_page(None, options()).await.unwrap()),
				json(
					r#"{ "resources": [{ "uri": "file:///a", "name": "a", "mimeType": "text/plain" }], "nextCursor": "2" }"#
				)
			);
			assert_eq!(
				to_json(&client.list_resources_page(Some("2"), options()).await.unwrap()),
				json(r#"{ "resources": [{ "uri": "file:///b", "name": "file:///b" }] }"#)
			);
			assert_eq!(
				to_json(&client.read_resource("file:///a", options()).await.unwrap()),
				json(r#"{ "contents": [{ "uri": "file:///a", "text": "hello" }] }"#)
			);

			server.set_result("resources/read", json!({ "contents": [{ "uri": "file:///a" }] }));
			let error = client.read_resource("file:///a", options()).await.unwrap_err();
			assert!(error.message.contains("Invalid contents in MCP resources/read result"));
			server.set_result("resources/list", json!({ "resources": [{ "name": "no uri" }] }));
			let error = client.list_resources(options()).await.unwrap_err();
			assert!(error.message.contains("Invalid entry in MCP resources/list result"));
			client.close().await.unwrap();
		});
	}

	#[test]
	fn returns_structured_tool_content_and_surfaces_json_rpc_errors() {
		run(|_| async {
			let (client, server) = connect().await;
			server.set_handler("tools/call", |request| {
				Box::pin(async move {
					if param(&request, "name") == Some(&json!("fail")) {
						return Err(McpError::new(
							1234.0,
							"tool failed",
							Some(json!({ "retryable": false })),
						));
					}
					let count = param(&request, "arguments").and_then(|args| args.get("count")).cloned();
					Ok(json!({
						"content": [{ "type": "text", "text": "ok" }],
						"structuredContent": { "count": count },
					}))
				})
			});
			assert_eq!(
				to_json(
					&client
						.call_tool("count", args(json!({ "count": 3 })), options())
						.await
						.unwrap()
				),
				json(r#"{ "content": [{ "type": "text", "text": "ok" }], "structuredContent": { "count": 3 } }"#)
			);
			let error = client.call_tool("fail", None, options()).await.unwrap_err();
			assert_eq!(error.name, "McpError");
			assert_eq!(McpError::code(&error), Some(1234.0));
			assert_eq!(error.message, "tool failed");
			assert_eq!(
				McpError::data(&error).map(|data| norm(&data)),
				Some(json(r#"{ "retryable": false }"#))
			);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn renews_the_timeout_on_progress() {
		run(|_| async {
			let (client, server) = connect().await;
			let progress_server = server.clone();
			server.set_handler("tools/call", move |request| {
				let token = request.params.as_ref().unwrap()["_meta"]["progressToken"].clone();
				let transport = progress_server.transport.clone();
				Box::pin(async move {
					let notify = transport.clone();
					pi_host::spawn_local(async move {
						sleep(40.0).await;
						let _ = send(
							&notify,
							JsonRpcMessage::notification(
								"notifications/progress",
								Some(json!({ "progressToken": token, "progress": 1, "total": 2 })),
							),
						)
						.await;
					});
					sleep(80.0).await;
					Ok(json!({ "content": [{ "type": "text", "text": "done" }] }))
				})
			});
			let progress: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
			let seen = progress.clone();
			let result = client.call_tool(
				"slow",
				Some(Map::new()),
				McpRequestOptions {
					timeout_ms: Some(50.0),
					on_progress: Some(Rc::new(move |notification| {
						seen.borrow_mut().push(to_json(notification));
						Ok(())
					})),
					..Default::default()
				},
			);
			assert_eq!(
				to_json(&result.await.unwrap()),
				json(r#"{ "content": [{ "type": "text", "text": "done" }] }"#)
			);
			assert_eq!(
				progress.borrow().clone(),
				vec![json(r#"{ "progressToken": 2, "progress": 1, "total": 2 }"#)]
			);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn cancels_aborted_and_timed_out_requests() {
		run(|_| async {
			let (client, server) = connect().await;
			server.set_handler("tools/call", |_| Box::pin(futures::future::pending()));
			let controller = AbortController::new();
			let aborted = client.call_tool(
				"wait",
				Some(Map::new()),
				McpRequestOptions {
					signal: Some(controller.signal()),
					..Default::default()
				},
			);
			// `controller.abort("stop")`: a thrown string is a `PiError` with an empty name.
			controller.abort_with(PiError::named("", "stop"));
			assert!(McpAbortError::is(&aborted.await.unwrap_err()));
			sleep(0.0).await;
			assert!(server.messages().contains(&json(
				r#"{ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 2, "reason": "stop" } }"#
			)));

			let timed_out = client.call_tool(
				"wait",
				Some(Map::new()),
				McpRequestOptions {
					timeout_ms: Some(5.0),
					..Default::default()
				},
			);
			assert!(McpTimeoutError::is(&timed_out.await.unwrap_err()));
			client.close().await.unwrap();
		});
	}

	#[test]
	fn reports_transport_errors_without_failing_pending_requests() {
		run(|_| async {
			let (client_transport, server) = create_server().await;
			let client = McpClient::new(client_options("1.0.0"));
			client.connect(Rc::new(client_transport.clone())).await.unwrap();
			let errors: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
			let seen = errors.clone();
			let _ = client.on_error(Rc::new(move |error| seen.borrow_mut().push(error.message.clone())));
			let respond = Gate::new();
			let gate = respond.clone();
			server.set_handler("tools/call", move |_| {
				let wait = gate.wait();
				Box::pin(async move {
					wait.await;
					Ok(json!({ "content": [] }))
				})
			});
			let call = client.call_tool("wait", None, options());
			sleep(0.0).await;
			client_transport.emit_error(&PiError::new("stray log line"));
			respond.release();
			assert_eq!(to_json(&call.await.unwrap()), json(r#"{ "content": [] }"#));
			assert_eq!(errors.borrow().clone(), vec!["stray log line".to_string()]);
			client.close().await.unwrap();
		});
	}

	#[test]
	fn accepts_servers_that_answer_with_an_older_protocol_version() {
		run(|_| async {
			let (client_transport, server) = create_server().await;
			server.set_result(
				"initialize",
				json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": { "name": "old-server", "version": "0.1.0" } }),
			);
			let client = McpClient::new(client_options("1.0.0"));
			client.connect(Rc::new(client_transport)).await.unwrap();
			assert_eq!(client.protocol_version().as_deref(), Some("2024-11-05"));
			client.close().await.unwrap();

			let (unsupported_transport, unsupported) = create_server().await;
			unsupported.set_result(
				"initialize",
				json!({ "protocolVersion": "1999-01-01", "capabilities": {}, "serverInfo": { "name": "ancient-server", "version": "0.1.0" } }),
			);
			let rejected = McpClient::new(client_options("1.0.0"));
			let error = rejected.connect(Rc::new(unsupported_transport)).await.unwrap_err();
			assert!(error.message.contains("unsupported protocol version"));
			assert_eq!(rejected.connection_state().as_str(), "closed");
		});
	}

	#[test]
	fn defaults_missing_tool_result_content_to_an_empty_list() {
		run(|_| async {
			let (client, server) = connect().await;
			server.set_result("tools/call", json!({ "structuredContent": { "ok": true } }));
			assert_eq!(
				to_json(&client.call_tool("structured", None, options()).await.unwrap()),
				json(r#"{ "content": [], "structuredContent": { "ok": true } }"#)
			);
			server.set_result("tools/call", json!({ "content": "not a list" }));
			let error = client.call_tool("broken", None, options()).await.unwrap_err();
			assert!(error.message.contains("Invalid MCP tools/call result"));
			client.close().await.unwrap();
		});
	}

	// #10416: MCP 2026-07-28 allows any JSON value as structured content.
	#[test]
	fn accepts_structured_content_that_is_not_an_object() {
		run(|_| async {
			let (client, server) = connect().await;
			for structured_content in [
				json!([{ "id": 1 }, { "id": 2 }]),
				Value::Null,
				json!("text"),
				json!(3),
				json!(false),
			] {
				server.set_result(
					"tools/call",
					json!({ "content": [], "structuredContent": structured_content }),
				);
				assert_eq!(
					to_json(&client.call_tool("structured", None, options()).await.unwrap()),
					norm(&json!({ "content": [], "structuredContent": structured_content }))
				);
			}
			client.close().await.unwrap();
		});
	}

	#[test]
	fn does_not_send_notifications_cancelled_for_a_timed_out_initialize() {
		run(|_| async {
			let (client_transport, server) = create_server().await;
			server.set_handler("initialize", |_| Box::pin(futures::future::pending()));
			let client = McpClient::new(McpClientOptions {
				request_timeout_ms: Some(5.0),
				..client_options("1.0.0")
			});
			let error = client.connect(Rc::new(client_transport)).await.unwrap_err();
			assert!(McpTimeoutError::is(&error));
			sleep(0.0).await;
			assert!(
				!server
					.messages()
					.iter()
					.any(|message| message.get("method") == Some(&json!("notifications/cancelled")))
			);
		});
	}

	#[test]
	fn notifies_close_listeners_once_when_the_transport_drops() {
		run(|_| async {
			let (client, server) = connect().await;
			let closed = Rc::new(Cell::new(0));
			let count = closed.clone();
			let _ = client.on_close(Rc::new(move || {
				count.set(count.get() + 1);
				Ok(())
			}));
			server.set_handler("tools/call", |_| Box::pin(futures::future::pending()));
			let pending = client.call_tool("wait", None, options());
			sleep(0.0).await;
			server.transport.close().await.unwrap();
			let error = pending.await.unwrap_err();
			assert!(error.message.contains("MCP connection closed"));
			assert_eq!(client.connection_state().as_str(), "closed");
			client.close().await.unwrap();
			assert_eq!(closed.get(), 1);
		});
	}

	#[test]
	fn answers_roots_list_and_dispatches_notifications() {
		run(|_| async {
			let (client_transport, server) = create_server().await;
			let client = McpClient::new(McpClientOptions {
				roots: Some(McpRoots::List(vec![Root {
					uri: "file:///workspace".into(),
					name: Some("workspace".into()),
				}])),
				..client_options("1.0.0")
			});
			client.connect(Rc::new(client_transport)).await.unwrap();
			let changed: Rc<RefCell<Vec<Option<Value>>>> = Rc::new(RefCell::new(Vec::new()));
			let seen = changed.clone();
			let _ = client.on_notification(
				"notifications/tools/list_changed",
				Rc::new(move |params| {
					seen.borrow_mut().push(params.cloned());
					Ok(())
				}),
			);
			send(&server.transport, JsonRpcMessage::request("roots", "roots/list", None))
				.await
				.unwrap();
			send(
				&server.transport,
				JsonRpcMessage::notification("notifications/tools/list_changed", None),
			)
			.await
			.unwrap();
			sleep(0.0).await;
			assert!(server.messages().contains(&json(
				r#"{ "jsonrpc": "2.0", "id": "roots", "result": { "roots": [{ "uri": "file:///workspace", "name": "workspace" }] } }"#
			)));
			assert_eq!(changed.borrow().clone(), vec![None]);
			client.close().await.unwrap();
		});
	}
}

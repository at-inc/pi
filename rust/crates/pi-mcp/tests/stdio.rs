//! Port of `packages/mcp/test/stdio.test.ts`. Needs the `native` feature and `node` on `PATH` (the fixtures are
//! the TS suite's `.mjs` servers).

#![cfg(feature = "native")]

use std::cell::RefCell;
use std::rc::Rc;

use pi_mcp::client::{ClientState, McpClient, McpClientOptions, McpRequestOptions};
use pi_mcp::transports::stdio::{StdioTransport, StdioTransportOptions};
use serde_json::{Value, json};

fn fixture(name: &str) -> String {
	format!(
		"{}/../../../packages/mcp/test/fixtures/{name}",
		env!("CARGO_MANIFEST_DIR")
	)
}

fn client() -> McpClient {
	McpClient::new(McpClientOptions {
		name: "stdio-test".into(),
		version: "1.0.0".into(),
		..Default::default()
	})
}

fn to_json<T: serde::Serialize>(value: &T) -> Value {
	serde_json::to_value(value).unwrap()
}

async fn sleep(ms: f64) {
	pi_host::sleep(ms, None).await.unwrap();
}

#[test]
fn connects_to_a_newline_delimited_mcp_server_and_captures_stderr() {
	pi_host::native::run(|| async {
		let stderr = Rc::new(RefCell::new(String::new()));
		let seen = stderr.clone();
		let transport = StdioTransport::new(StdioTransportOptions {
			command: "node".into(),
			args: Some(vec![fixture("stdio-server.mjs")]),
			on_stderr: Some(Rc::new(move |chunk| seen.borrow_mut().push_str(chunk))),
			..Default::default()
		});
		let client = client();
		client.connect(Rc::new(transport.clone())).await.unwrap();
		assert_eq!(
			to_json(&client.list_tools(McpRequestOptions::default()).await.unwrap()),
			json!([{ "name": "echo", "inputSchema": { "type": "object" } }])
		);
		let args = json!({ "text": "hello" }).as_object().cloned();
		assert_eq!(
			to_json(
				&client
					.call_tool("echo", args, McpRequestOptions::default())
					.await
					.unwrap()
			),
			json!({ "content": [{ "type": "text", "text": "hello" }] })
		);
		assert!(transport.pid().is_some());
		sleep(10.0).await;
		assert!(stderr.borrow().contains("stdio fixture ready"), "{}", stderr.borrow());
		assert!(transport.stderr().contains("stdio fixture ready"));
		client.close().await.unwrap();
		assert_eq!(client.connection_state(), ClientState::Closed);
	});
}

#[cfg(unix)]
#[test]
fn kills_a_server_that_ignores_shutdown_including_its_children() {
	use nix::sys::signal::kill;
	use nix::unistd::Pid;

	pi_host::native::run(|| async {
		let transport = StdioTransport::new(StdioTransportOptions {
			command: "node".into(),
			args: Some(vec![fixture("stubborn-server.mjs")]),
			close_timeout_ms: Some(100.0),
			..Default::default()
		});
		let client = client();
		client.connect(Rc::new(transport.clone())).await.unwrap();
		let pattern = regex::Regex::new(r"grandchild (\d+)").unwrap();
		let mut grandchild: Option<i32> = None;
		for _ in 0..100 {
			grandchild = pattern
				.captures(&transport.stderr())
				.and_then(|captures| captures[1].parse().ok());
			if grandchild.is_some() {
				break;
			}
			sleep(10.0).await;
		}
		let grandchild = grandchild.expect("grandchild pid on stderr");

		let started_at = pi_host::now_ms();
		client.close().await.unwrap();
		assert!(pi_host::now_ms() - started_at < 5_000.0);
		let mut alive = true;
		for _ in 0..100 {
			if kill(Pid::from_raw(grandchild), None).is_err() {
				alive = false;
				break;
			}
			sleep(20.0).await;
		}
		assert!(!alive, "grandchild {grandchild} is still alive");
	});
}

//! Exports of `transports/stdio`: `new StdioTransport(options)` over the Rust transport, which spawns the server
//! through the host's `spawnProcess`.

use std::rc::Rc;

use pi_mcp::transports::stdio::{StderrListener, StdioStderr};
use pi_mcp::{StdioTransport, StdioTransportOptions};
use wasm_bindgen::prelude::*;

use super::transport::{McpTransportHandle, TransportKind};
use super::{field, function, optional};

/// `new StdioTransport(options)`.
#[wasm_bindgen(js_name = mcpCreateStdioTransport)]
pub fn create_stdio_transport(options: JsValue) -> Result<McpTransportHandle, JsValue> {
	let on_stderr = function(&options, "onStderr").map(|listener| {
		Rc::new(move |chunk: &str| {
			let _ = listener.call1(&JsValue::UNDEFINED, &chunk.into());
		}) as StderrListener
	});
	let stderr = field(&options, "stderr")
		.as_string()
		.map(|stderr| match stderr.as_str() {
			"inherit" => StdioStderr::Inherit,
			_ => StdioStderr::Pipe,
		});
	let transport = StdioTransport::new(StdioTransportOptions {
		command: crate::options::arg(field(&options, "command"))?,
		args: optional(&options, "args")?,
		cwd: optional(&options, "cwd")?,
		env: optional(&options, "env")?,
		inherit_env: optional(&options, "inheritEnv")?,
		stderr,
		on_stderr,
		max_message_bytes: optional(&options, "maxMessageBytes")?,
		max_stderr_bytes: optional(&options, "maxStderrBytes")?,
		close_timeout_ms: optional(&options, "closeTimeoutMs")?,
	});
	Ok(McpTransportHandle::new(TransportKind::Stdio(transport)))
}

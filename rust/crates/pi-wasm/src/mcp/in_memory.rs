//! Exports of `transports/in-memory`: `new InMemoryTransport()` over the Rust transport.

use pi_mcp::testing::InMemoryTransport;
use wasm_bindgen::prelude::*;

use super::transport::{McpTransportHandle, TransportKind};

/// `new InMemoryTransport()`.
#[wasm_bindgen(js_name = mcpCreateInMemoryTransport)]
pub fn create_in_memory_transport() -> McpTransportHandle {
	McpTransportHandle::new(TransportKind::InMemory(InMemoryTransport::new()))
}

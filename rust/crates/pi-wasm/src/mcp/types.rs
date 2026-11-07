//! Exports of `protocol/types`: the protocol version constants.

use wasm_bindgen::prelude::*;

/// `LATEST_PROTOCOL_VERSION`.
#[wasm_bindgen(js_name = mcpLatestProtocolVersion)]
pub fn latest_protocol_version() -> String {
	pi_mcp::LATEST_PROTOCOL_VERSION.into()
}

/// `SUPPORTED_PROTOCOL_VERSIONS`.
#[wasm_bindgen(js_name = mcpSupportedProtocolVersions)]
pub fn supported_protocol_versions() -> Vec<String> {
	pi_mcp::SUPPORTED_PROTOCOL_VERSIONS.map(String::from).to_vec()
}

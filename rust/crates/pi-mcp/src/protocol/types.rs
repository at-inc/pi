//! Port of `packages/mcp/src/protocol/types.ts`.
//! PORT-STATUS: ported
//!
//! Types the server sends keep unknown fields in a flattened `extra` map, since the TS passes server objects
//! through untouched. Serializing one emits the known fields first, then `extra`, so key order can differ
//! from the server's. `SupportedProtocolVersion` is a `&str`: the TS type is only the union of
//! `SUPPORTED_PROTOCOL_VERSIONS`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::content::{BlobResourceContents, ContentAnnotations, TextResourceContents};
use super::jsonrpc::JsonRpcId;

pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";
/// Versions the client accepts from a server. Servers that do not support the requested version answer
/// with their own latest one, so older versions stay accepted for servers built on older SDKs.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] = [LATEST_PROTOCOL_VERSION, "2025-06-18", "2025-03-26", "2024-11-05"];
pub type SupportedProtocolVersion = &'static str;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Implementation {
	pub name: String,
	pub version: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Root {
	pub uri: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChangedCapability {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub list_changed: Option<bool>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesCapability {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub subscribe: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub list_changed: Option<bool>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientCapabilities {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub experimental: Option<Map<String, Value>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub roots: Option<ListChangedCapability>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sampling: Option<Map<String, Value>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub elicitation: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilities {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub experimental: Option<Map<String, Value>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub logging: Option<Map<String, Value>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub prompts: Option<ListChangedCapability>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resources: Option<ResourcesCapability>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tools: Option<ListChangedCapability>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub completions: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
	pub protocol_version: String,
	pub capabilities: ClientCapabilities,
	pub client_info: Implementation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
	pub protocol_version: String,
	pub capabilities: ServerCapabilities,
	pub server_info: Implementation,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub instructions: Option<String>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressNotification {
	pub progress_token: JsonRpcId,
	pub progress: f64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub total: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub message: Option<String>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelledNotification {
	pub request_id: JsonRpcId,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub read_only_hint: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub destructive_hint: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub idempotent_hint: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub open_world_hint: Option<bool>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolTaskSupport {
	#[serde(rename = "forbidden")]
	Forbidden,
	#[serde(rename = "optional")]
	Optional,
	#[serde(rename = "required")]
	Required,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolExecution {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub task_support: Option<ToolTaskSupport>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tool {
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	pub input_schema: Map<String, Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub output_schema: Option<Map<String, Value>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ToolAnnotations>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub execution: Option<ToolExecution>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListToolsResult {
	pub tools: Vec<Tool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_cursor: Option<String>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
}

/// A resource a server lists in `resources/list`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
	pub uri: String,
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mime_type: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub size: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ContentAnnotations>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// A family of resources, addressed by an RFC 6570 URI template, from `resources/templates/list`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceTemplate {
	pub uri_template: String,
	pub name: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub title: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub description: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mime_type: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ContentAnnotations>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourcesResult {
	pub resources: Vec<Resource>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_cursor: Option<String>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListResourceTemplatesResult {
	pub resource_templates: Vec<ResourceTemplate>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub next_cursor: Option<String>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
}

/// `TextResourceContents | BlobResourceContents`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResourceContents {
	Text(TextResourceContents),
	Blob(BlobResourceContents),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReadResourceResult {
	pub contents: Vec<ResourceContents>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

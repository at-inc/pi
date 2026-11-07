//! Port of `packages/mcp/src/protocol/content.ts`.
//! PORT-STATUS: ported
//!
//! `ContentBlock` adds an `Unknown` variant for blocks whose `type` is not one of the five known ones, or whose
//! fields do not have the TS types: the TS passes such blocks through and `toLlmContent` turns them into a
//! placeholder. A malformed known block (for example `{ type: "text" }` without `text`) therefore becomes the
//! `[unsupported MCP content text]` placeholder instead of a text block with an undefined `text`.
//! `ContentAnnotations.audience` is a list of strings rather than a `"user" | "assistant"` enum, so a server
//! that sends another role does not make the whole block unreadable.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use super::jsonrpc::present;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentAnnotations {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub audience: Option<Vec<String>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub priority: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub last_modified: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
	pub text: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ContentAnnotations>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageContent {
	pub data: String,
	pub mime_type: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ContentAnnotations>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioContent {
	pub data: String,
	pub mime_type: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ContentAnnotations>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceLinkContent {
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextResourceContents {
	pub uri: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mime_type: Option<String>,
	pub text: String,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobResourceContents {
	pub uri: String,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub mime_type: Option<String>,
	pub blob: String,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// `TextResourceContents | BlobResourceContents`; `"text" in resource` picks the text variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EmbeddedResource {
	Text(TextResourceContents),
	Blob(BlobResourceContents),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbeddedResourceContent {
	pub resource: EmbeddedResource,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub annotations: Option<ContentAnnotations>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ContentBlock {
	Text(TextContent),
	Image(ImageContent),
	Audio(AudioContent),
	ResourceLink(ResourceLinkContent),
	Resource(EmbeddedResourceContent),
	/// A block of another type, or one whose fields do not match its type, kept as sent.
	Unknown(Value),
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum TaggedRef<'a> {
	#[serde(rename = "text")]
	Text(&'a TextContent),
	#[serde(rename = "image")]
	Image(&'a ImageContent),
	#[serde(rename = "audio")]
	Audio(&'a AudioContent),
	#[serde(rename = "resource_link")]
	ResourceLink(&'a ResourceLinkContent),
	#[serde(rename = "resource")]
	Resource(&'a EmbeddedResourceContent),
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum Tagged {
	#[serde(rename = "text")]
	Text(TextContent),
	#[serde(rename = "image")]
	Image(ImageContent),
	#[serde(rename = "audio")]
	Audio(AudioContent),
	#[serde(rename = "resource_link")]
	ResourceLink(ResourceLinkContent),
	#[serde(rename = "resource")]
	Resource(EmbeddedResourceContent),
}

impl Serialize for ContentBlock {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		match self {
			ContentBlock::Text(block) => TaggedRef::Text(block).serialize(serializer),
			ContentBlock::Image(block) => TaggedRef::Image(block).serialize(serializer),
			ContentBlock::Audio(block) => TaggedRef::Audio(block).serialize(serializer),
			ContentBlock::ResourceLink(block) => TaggedRef::ResourceLink(block).serialize(serializer),
			ContentBlock::Resource(block) => TaggedRef::Resource(block).serialize(serializer),
			ContentBlock::Unknown(block) => block.serialize(serializer),
		}
	}
}

impl<'de> Deserialize<'de> for ContentBlock {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		let value = Value::deserialize(deserializer)?;
		Ok(match serde_json::from_value::<Tagged>(value.clone()) {
			Ok(Tagged::Text(block)) => ContentBlock::Text(block),
			Ok(Tagged::Image(block)) => ContentBlock::Image(block),
			Ok(Tagged::Audio(block)) => ContentBlock::Audio(block),
			Ok(Tagged::ResourceLink(block)) => ContentBlock::ResourceLink(block),
			Ok(Tagged::Resource(block)) => ContentBlock::Resource(block),
			Err(_) => ContentBlock::Unknown(value),
		})
	}
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallToolResult {
	pub content: Vec<ContentBlock>,
	/// Any JSON value, including arrays and `null` (MCP 2026-07-28; earlier revisions allowed only objects).
	#[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
	pub structured_content: Option<Value>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub is_error: Option<bool>,
	#[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
	pub meta: Option<Map<String, Value>>,
	#[serde(flatten)]
	pub extra: Map<String, Value>,
}

/// Tool result content in the shape LLM APIs accept: text and base64 images. Matches the
/// `TextContent` and `ImageContent` types of `@at-inc/pi-ai`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LlmContent {
	#[serde(rename = "text")]
	Text { text: String },
	#[serde(rename = "image")]
	Image {
		data: String,
		#[serde(rename = "mimeType")]
		mime_type: String,
	},
}

/// `String(value)` for a JSON value.
pub(crate) fn js_string(value: Option<&Value>) -> String {
	match value {
		None => "undefined".to_string(),
		Some(Value::Null) => "null".to_string(),
		Some(Value::String(text)) => text.clone(),
		Some(Value::Bool(flag)) => flag.to_string(),
		Some(Value::Number(number)) => pi_host::js::number_to_string(number.as_f64().unwrap_or(f64::NAN)),
		Some(Value::Array(items)) => items
			.iter()
			.map(|item| match item {
				Value::Null => String::new(),
				item => js_string(Some(item)),
			})
			.collect::<Vec<_>>()
			.join(","),
		Some(Value::Object(_)) => "[object Object]".to_string(),
	}
}

fn block_to_llm_content(block: &ContentBlock) -> LlmContent {
	match block {
		ContentBlock::Text(block) => LlmContent::Text {
			text: block.text.clone(),
		},
		ContentBlock::Image(block) => LlmContent::Image {
			data: block.data.clone(),
			mime_type: block.mime_type.clone(),
		},
		ContentBlock::Audio(block) => LlmContent::Text {
			text: format!("[audio {} omitted]", block.mime_type),
		},
		ContentBlock::ResourceLink(block) => LlmContent::Text {
			text: format!("{}: {}", block.name, block.uri),
		},
		ContentBlock::Resource(block) => match &block.resource {
			EmbeddedResource::Text(resource) => LlmContent::Text {
				text: resource.text.clone(),
			},
			EmbeddedResource::Blob(resource) => {
				if let Some(mime_type) = resource
					.mime_type
					.as_ref()
					.filter(|mime_type| mime_type.starts_with("image/"))
				{
					return LlmContent::Image {
						data: resource.blob.clone(),
						mime_type: mime_type.clone(),
					};
				}
				LlmContent::Text {
					text: format!(
						"[binary resource {} ({}) omitted]",
						resource.uri,
						resource.mime_type.as_deref().unwrap_or("unknown type")
					),
				}
			}
		},
		ContentBlock::Unknown(block) => LlmContent::Text {
			text: format!("[unsupported MCP content {}]", js_string(block.get("type"))),
		},
	}
}

/// Convert a tool result to text and image content for a model. Text and images pass through,
/// embedded text resources become text, embedded image resources become images, and other blocks
/// (audio, resource links, binary resources) become a short text placeholder. A result without
/// content blocks but with `structuredContent` becomes its JSON, since servers should, but do not
/// always, mirror structured results as text.
pub fn to_llm_content(result: &CallToolResult) -> Vec<LlmContent> {
	let mut content: Vec<LlmContent> = result.content.iter().map(block_to_llm_content).collect();
	if content.is_empty()
		&& let Some(structured_content) = &result.structured_content
	{
		content.push(LlmContent::Text {
			text: pi_host::js::json_stringify_pretty(structured_content, 2),
		});
	}
	content
}

//! Port of `packages/ai/src/providers/model-schema.ts`.
//! PORT-STATUS: ported
//!
//! The schemas are JSON values (see `schema_builders.rs`). Each `Static<typeof XSchema>` type alias becomes the
//! Rust type of the values the schema accepts.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::schema_builders::{Properties, open_object as object, opt};

/// Thinking level requested from a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThinkingLevel {
	#[serde(rename = "minimal")]
	Minimal,
	#[serde(rename = "low")]
	Low,
	#[serde(rename = "medium")]
	Medium,
	#[serde(rename = "high")]
	High,
	#[serde(rename = "xhigh")]
	Xhigh,
	#[serde(rename = "max")]
	Max,
}

impl ThinkingLevel {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			ThinkingLevel::Minimal => "minimal",
			ThinkingLevel::Low => "low",
			ThinkingLevel::Medium => "medium",
			ThinkingLevel::High => "high",
			ThinkingLevel::Xhigh => "xhigh",
			ThinkingLevel::Max => "max",
		}
	}
}

/// Thinking level of a model, including `off`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelThinkingLevel {
	#[serde(rename = "off")]
	Off,
	#[serde(rename = "minimal")]
	Minimal,
	#[serde(rename = "low")]
	Low,
	#[serde(rename = "medium")]
	Medium,
	#[serde(rename = "high")]
	High,
	#[serde(rename = "xhigh")]
	Xhigh,
	#[serde(rename = "max")]
	Max,
}

impl ModelThinkingLevel {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			ModelThinkingLevel::Off => "off",
			ModelThinkingLevel::Minimal => "minimal",
			ModelThinkingLevel::Low => "low",
			ModelThinkingLevel::Medium => "medium",
			ModelThinkingLevel::High => "high",
			ModelThinkingLevel::Xhigh => "xhigh",
			ModelThinkingLevel::Max => "max",
		}
	}
}

impl From<ThinkingLevel> for ModelThinkingLevel {
	fn from(level: ThinkingLevel) -> Self {
		match level {
			ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
			ThinkingLevel::Low => ModelThinkingLevel::Low,
			ThinkingLevel::Medium => ModelThinkingLevel::Medium,
			ThinkingLevel::High => ModelThinkingLevel::High,
			ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
			ThinkingLevel::Max => ModelThinkingLevel::Max,
		}
	}
}

/// Input modality a model accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModelInputModality {
	#[serde(rename = "text")]
	Text,
	#[serde(rename = "image")]
	Image,
}

impl ModelInputModality {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			ModelInputModality::Text => "text",
			ModelInputModality::Image => "image",
		}
	}
}

/// Prompt cache retention preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheRetention {
	#[serde(rename = "none")]
	None,
	#[serde(rename = "short")]
	Short,
	#[serde(rename = "long")]
	Long,
}

impl CacheRetention {
	/// The wire value.
	pub fn as_str(self) -> &'static str {
		match self {
			CacheRetention::None => "none",
			CacheRetention::Short => "short",
			CacheRetention::Long => "long",
		}
	}
}

pub const THINKING_LEVELS: [ThinkingLevel; 6] = [
	ThinkingLevel::Minimal,
	ThinkingLevel::Low,
	ThinkingLevel::Medium,
	ThinkingLevel::High,
	ThinkingLevel::Xhigh,
	ThinkingLevel::Max,
];
pub const MODEL_THINKING_LEVELS: [ModelThinkingLevel; 7] = [
	ModelThinkingLevel::Off,
	ModelThinkingLevel::Minimal,
	ModelThinkingLevel::Low,
	ModelThinkingLevel::Medium,
	ModelThinkingLevel::High,
	ModelThinkingLevel::Xhigh,
	ModelThinkingLevel::Max,
];
pub const MODEL_INPUT_MODALITIES: [ModelInputModality; 2] = [ModelInputModality::Text, ModelInputModality::Image];
pub const CACHE_RETENTIONS: [CacheRetention; 3] = [CacheRetention::None, CacheRetention::Short, CacheRetention::Long];
pub const MODEL_PROMPT_CACHE_RETENTIONS: [CacheRetention; 2] = [CacheRetention::Short, CacheRetention::Long];

pub static THINKING_LEVEL_SCHEMA: LazyLock<Value> =
	LazyLock::new(|| json!({ "enum": THINKING_LEVELS.map(ThinkingLevel::as_str) }));
pub static MODEL_THINKING_LEVEL_SCHEMA: LazyLock<Value> =
	LazyLock::new(|| json!({ "enum": MODEL_THINKING_LEVELS.map(ModelThinkingLevel::as_str) }));
pub static MODEL_INPUT_MODALITY_SCHEMA: LazyLock<Value> =
	LazyLock::new(|| json!({ "enum": MODEL_INPUT_MODALITIES.map(ModelInputModality::as_str) }));
pub static CACHE_RETENTION_SCHEMA: LazyLock<Value> =
	LazyLock::new(|| json!({ "enum": CACHE_RETENTIONS.map(CacheRetention::as_str) }));

fn thinking_level_map_value_schema() -> Value {
	json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
}

pub static THINKING_LEVEL_MAP_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	let properties: Properties = ["off", "minimal", "low", "medium", "high", "xhigh", "max"]
		.into_iter()
		.map(|level| (level.to_string(), opt(thinking_level_map_value_schema())))
		.collect();
	object(
		&properties,
		json!({
			"description": "Provider value sent for each thinking level. null marks a level unsupported. Levels the model does not map use the provider's default value through high; xhigh and max are unsupported.",
		}),
	)
});

fn prompt_cache_lifetime_schema() -> Value {
	json!({ "type": "number", "exclusiveMinimum": 0 })
}

pub static MODEL_PROMPT_CACHE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	let properties: Properties = [
		("short".to_string(), opt(prompt_cache_lifetime_schema())),
		("long".to_string(), opt(prompt_cache_lifetime_schema())),
	]
	.into_iter()
	.collect();
	object(
		&properties,
		json!({
			"description": "Best-effort prompt cache lifetime in seconds for the short and long retention tiers. Pi warms only tiers with a known lifetime.",
		}),
	)
});

fn model_cost_rates_properties() -> Properties {
	[
		(
			"input",
			json!({ "type": "number", "description": "Input cost in USD per million tokens." }),
		),
		(
			"output",
			json!({ "type": "number", "description": "Output cost in USD per million tokens." }),
		),
		(
			"cacheRead",
			json!({ "type": "number", "description": "Cache-read cost in USD per million tokens." }),
		),
		(
			"cacheWrite",
			json!({ "type": "number", "description": "Cache-write cost in USD per million tokens." }),
		),
	]
	.into_iter()
	.map(|(name, schema)| (name.to_string(), schema.into()))
	.collect()
}

pub static MODEL_COST_RATES_SCHEMA: LazyLock<Value> =
	LazyLock::new(|| object(&model_cost_rates_properties(), json!({})));
pub static MODEL_COST_TIER_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	let mut properties = Properties::new();
	properties.insert(
		"inputTokensAbove".into(),
		json!({
			"type": "number",
			"description": "Use this tier when total request input exceeds this token count.",
		})
		.into(),
	);
	properties.extend(model_cost_rates_properties());
	object(&properties, json!({}))
});
pub static MODEL_COST_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	let mut properties = model_cost_rates_properties();
	properties.insert(
		"tiers".into(),
		opt(json!({
			"type": "array",
			"items": MODEL_COST_TIER_SCHEMA.clone(),
			"description": "Request-wide pricing tiers. The highest matching input threshold applies to the full request.",
		})),
	);
	object(&properties, json!({}))
});

fn model_image_resize_properties() -> Properties {
	[
		("maxWidth", json!({ "type": "integer", "minimum": 1 })),
		("maxHeight", json!({ "type": "integer", "minimum": 1 })),
		(
			"maxBytes",
			json!({ "type": "integer", "minimum": 1, "description": "Maximum base64-encoded payload size in bytes." }),
		),
		(
			"jpegQuality",
			json!({ "type": "integer", "minimum": 1, "maximum": 100 }),
		),
	]
	.into_iter()
	.map(|(name, schema)| (name.to_string(), opt(schema)))
	.collect()
}

fn model_image_resize_options(options: Value) -> Value {
	object(&model_image_resize_properties(), options)
}

pub static MODEL_IMAGE_RESIZE_OPTIONS_SCHEMA: LazyLock<Value> = LazyLock::new(|| model_image_resize_options(json!({})));

pub static MODEL_IMAGE_INPUT_LIMITS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	let properties: Properties = [
		(
			"resize",
			opt(model_image_resize_options(json!({
				"description": "Cache-safe resize profile applied before a new image enters conversation history.",
			}))),
		),
		(
			"maxPerMessage",
			opt(
				json!({ "type": "integer", "minimum": 1, "description": "Maximum images accepted in one provider message." }),
			),
		),
		(
			"maxPerRequest",
			opt(
				json!({ "type": "integer", "minimum": 1, "description": "Maximum images accepted across one provider request." }),
			),
		),
	]
	.into_iter()
	.map(|(name, property)| (name.to_string(), property))
	.collect();
	object(&properties, json!({}))
});

pub static MODEL_INPUT_LIMITS_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	let properties: Properties = [
		(
			"maxRequestBytes",
			opt(
				json!({ "type": "integer", "minimum": 1, "description": "Maximum serialized provider request size in bytes." }),
			),
		),
		("images", opt(MODEL_IMAGE_INPUT_LIMITS_SCHEMA.clone())),
	]
	.into_iter()
	.map(|(name, property)| (name.to_string(), property))
	.collect();
	object(&properties, json!({}))
});

/// `Static<typeof ThinkingLevelMapSchema>`: the provider value per thinking level. `Some(None)` is an explicit `null`
/// (the level is unsupported); `None` leaves the level unmapped.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ThinkingLevelMap {
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub off: Option<Option<String>>,
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub minimal: Option<Option<String>>,
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub low: Option<Option<String>>,
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub medium: Option<Option<String>>,
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub high: Option<Option<String>>,
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub xhigh: Option<Option<String>>,
	#[serde(
		default,
		deserialize_with = "pi_host::js::double_option",
		skip_serializing_if = "Option::is_none"
	)]
	pub max: Option<Option<String>>,
}

impl ThinkingLevelMap {
	/// `map[level]`: `None` when the level is absent, `Some(None)` when it is `null`.
	pub fn get(&self, level: ModelThinkingLevel) -> Option<Option<&str>> {
		let value = match level {
			ModelThinkingLevel::Off => &self.off,
			ModelThinkingLevel::Minimal => &self.minimal,
			ModelThinkingLevel::Low => &self.low,
			ModelThinkingLevel::Medium => &self.medium,
			ModelThinkingLevel::High => &self.high,
			ModelThinkingLevel::Xhigh => &self.xhigh,
			ModelThinkingLevel::Max => &self.max,
		};
		value.as_ref().map(Option::as_deref)
	}
}

/// `Static<typeof ModelPromptCacheSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPromptCache {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub short: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub long: Option<f64>,
}

/// `Static<typeof ModelCostRatesSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostRates {
	pub input: f64,
	pub output: f64,
	pub cache_read: f64,
	pub cache_write: f64,
}

/// `Static<typeof ModelCostTierSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCostTier {
	pub input_tokens_above: f64,
	pub input: f64,
	pub output: f64,
	pub cache_read: f64,
	pub cache_write: f64,
}

/// `Static<typeof ModelCostSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
	pub input: f64,
	pub output: f64,
	pub cache_read: f64,
	pub cache_write: f64,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tiers: Option<Vec<ModelCostTier>>,
}

/// `Static<typeof ModelImageResizeOptionsSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelImageResizeOptions {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_width: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_height: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_bytes: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub jpeg_quality: Option<f64>,
}

/// `Static<typeof ModelImageInputLimitsSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelImageInputLimits {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub resize: Option<ModelImageResizeOptions>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_per_message: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_per_request: Option<f64>,
}

/// `Static<typeof ModelInputLimitsSchema>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInputLimits {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_request_bytes: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub images: Option<ModelImageInputLimits>,
}

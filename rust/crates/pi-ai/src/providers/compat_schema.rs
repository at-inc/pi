//! Port of `packages/ai/src/providers/compat-schema.ts`.
//! PORT-STATUS: ported
//!
//! The schemas are JSON values (see `schema_builders.rs`). Each `Static<typeof XSchema>` type becomes a Rust
//! struct or enum of the values the schema accepts. `OpenRouterRouting` and `VercelGatewayRouting` stay raw JSON
//! objects because providers send them verbatim; their schemas document the known keys. The type-level helpers
//! `UnionToIntersection` and `MergedCompatProperties` are dropped. A `chatTemplateKwargs` variable value
//! (`ChatTemplateVar`) stays raw JSON, because TS resolves any non-null object as a variable even when the schema
//! would reject it.

use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use pi_host::{PiError, PiResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::model_schema::{MODEL_COST_SCHEMA, ModelCost};
use super::schema_builders::{Properties, Property, open_object, opt, string_literal, string_record};

fn session_affinity_format(options: Value) -> Value {
	with_options(
		json!({
			"anyOf": [
				string_literal("openai", json!({})),
				string_literal("openai-nosession", json!({})),
				string_literal("openrouter", json!({})),
			],
		}),
		options,
	)
}

pub static SESSION_AFFINITY_FORMAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	session_affinity_format(json!({
		"description": "Session-affinity header format used to route related requests consistently.",
	}))
});

fn thinking_token_budget_field(options: Value) -> Value {
	with_options(
		json!({
			"anyOf": [
				string_literal("thinking_token_budget", json!({})),
				string_literal("thinking_budget", json!({})),
				string_literal("thinking_budget_tokens", json!({})),
			],
		}),
		options,
	)
}

pub static THINKING_TOKEN_BUDGET_FIELD_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	thinking_token_budget_field(json!({
		"description": "Top-level request field name used by OpenAI-compatible endpoints to cap reasoning tokens. \"thinking_token_budget\" is used by vLLM, \"thinking_budget\" by Qwen, DashScope, or SGLang, and \"thinking_budget_tokens\" by llama.cpp.",
	}))
});

pub static CHAT_TEMPLATE_KWARG_VALUE_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	json!({
		"anyOf": [
			{ "type": "string" },
			{ "type": "number" },
			{ "type": "boolean" },
			{ "type": "null" },
			open_object(
				&properties([
					(
						"$var",
						json!({
							"anyOf": [
								string_literal("thinking.enabled", json!({})),
								string_literal("thinking.effort", json!({})),
								string_literal("thinking.budget", json!({})),
							],
						})
						.into(),
					),
					("omitWhenOff", opt(json!({ "type": "boolean" }))),
				]),
				json!({}),
			),
		],
	})
});

fn percentile_cutoffs(metric: &str) -> Value {
	open_object(
		&properties([
			(
				"p50",
				opt(json!({ "type": "number", "description": format!("{metric} at the 50th percentile.") })),
			),
			(
				"p75",
				opt(json!({ "type": "number", "description": format!("{metric} at the 75th percentile.") })),
			),
			(
				"p90",
				opt(json!({ "type": "number", "description": format!("{metric} at the 90th percentile.") })),
			),
			(
				"p99",
				opt(json!({ "type": "number", "description": format!("{metric} at the 99th percentile.") })),
			),
		]),
		json!({}),
	)
}

pub static OPEN_ROUTER_ROUTING_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&properties([
			(
				"allow_fallbacks",
				opt(json!({
					"type": "boolean",
					"description": "Whether to allow backup providers to serve requests.",
					"default": true,
				})),
			),
			(
				"require_parameters",
				opt(json!({
					"type": "boolean",
					"description": "Whether to filter providers to only those that support all parameters in the request.",
					"default": false,
				})),
			),
			(
				"data_collection",
				opt(json!({
					"anyOf": [string_literal("deny", json!({})), string_literal("allow", json!({}))],
					"description": "Data collection setting. \"allow\": allow providers that may store or train on data. \"deny\": only use providers that do not collect user data.",
					"default": "allow",
				})),
			),
			(
				"zdr",
				opt(json!({
					"type": "boolean",
					"description": "Whether to restrict routing to only ZDR (Zero Data Retention) endpoints.",
				})),
			),
			(
				"enforce_distillable_text",
				opt(json!({
					"type": "boolean",
					"description": "Whether to restrict routing to only models that allow text distillation.",
				})),
			),
			(
				"order",
				opt(json!({
					"type": "array",
					"items": { "type": "string" },
					"description": "An ordered list of provider names or slugs to try in sequence, falling back to the next if unavailable.",
				})),
			),
			(
				"only",
				opt(json!({
					"type": "array",
					"items": { "type": "string" },
					"description": "List of provider names or slugs to exclusively allow for this request.",
				})),
			),
			(
				"ignore",
				opt(json!({
					"type": "array",
					"items": { "type": "string" },
					"description": "List of provider names or slugs to skip for this request.",
				})),
			),
			(
				"quantizations",
				opt(json!({
					"type": "array",
					"items": { "type": "string" },
					"description": "A list of quantization levels to filter providers by, for example [\"fp16\", \"bf16\", \"fp8\", \"fp6\", \"int8\", \"int4\", \"fp4\", \"fp32\"].",
				})),
			),
			(
				"sort",
				opt(json!({
					"anyOf": [
						{ "type": "string" },
						open_object(
							&properties([
								(
									"by",
									opt(json!({
										"type": "string",
										"description": "The sorting metric, such as \"price\", \"throughput\", or \"latency\".",
									})),
								),
								(
									"partition",
									opt(json!({
										"anyOf": [{ "type": "string" }, { "type": "null" }],
										"description": "Partitioning strategy: \"model\" or \"none\".",
										"default": "model",
									})),
								),
							]),
							json!({}),
						),
					],
					"description": "Sorting strategy. Can be a string such as \"price\", \"throughput\", or \"latency\", or an object.",
				})),
			),
			(
				"max_price",
				opt(open_object(
					&properties([
						(
							"prompt",
							opt(json!({
								"anyOf": [{ "type": "number" }, { "type": "string" }],
								"description": "Price per million prompt tokens.",
							})),
						),
						(
							"completion",
							opt(json!({
								"anyOf": [{ "type": "number" }, { "type": "string" }],
								"description": "Price per million completion tokens.",
							})),
						),
						(
							"image",
							opt(json!({
								"anyOf": [{ "type": "number" }, { "type": "string" }],
								"description": "Price per image.",
							})),
						),
						(
							"audio",
							opt(json!({
								"anyOf": [{ "type": "number" }, { "type": "string" }],
								"description": "Price per audio unit.",
							})),
						),
						(
							"request",
							opt(json!({
								"anyOf": [{ "type": "number" }, { "type": "string" }],
								"description": "Price per request.",
							})),
						),
					]),
					json!({ "description": "Maximum price per million tokens in USD." }),
				)),
			),
			(
				"preferred_min_throughput",
				opt(json!({
					"anyOf": [{ "type": "number" }, percentile_cutoffs("Minimum tokens per second")],
					"description": "Preferred minimum throughput in tokens per second. A number applies to p50.",
				})),
			),
			(
				"preferred_max_latency",
				opt(json!({
					"anyOf": [{ "type": "number" }, percentile_cutoffs("Maximum latency in seconds")],
					"description": "Preferred maximum latency in seconds. A number applies to p50.",
				})),
			),
		]),
		json!({
			"description": "OpenRouter provider routing preferences. Controls which upstream providers OpenRouter routes requests to. Sent as the provider field in the OpenRouter API request body. See https://openrouter.ai/docs/guides/routing/provider-selection.",
		}),
	)
});

pub static VERCEL_GATEWAY_ROUTING_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&properties([
			(
				"only",
				opt(json!({
					"type": "array",
					"items": { "type": "string" },
					"description": "List of provider slugs to exclusively use for this request, for example [\"bedrock\", \"anthropic\"].",
				})),
			),
			(
				"order",
				opt(json!({
					"type": "array",
					"items": { "type": "string" },
					"description": "List of provider slugs to try in order, for example [\"anthropic\", \"openai\"].",
				})),
			),
		]),
		json!({
			"description": "Vercel AI Gateway routing preferences. Controls which upstream providers the gateway routes requests to. See https://vercel.com/docs/ai-gateway/models-and-providers/provider-options.",
		}),
	)
});

pub static ANTHROPIC_ALLOWED_FALLBACK_MODEL_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&properties([
			("provider", json!({ "type": "string", "minLength": 1 }).into()),
			("model", json!({ "type": "string", "minLength": 1 }).into()),
			("cost", MODEL_COST_SCHEMA.clone().into()),
		]),
		json!({ "description": "An Anthropic server-side refusal fallback model with local pricing metadata." }),
	)
});

fn optional_compat_boolean(options: Value) -> Property {
	opt(with_options(json!({ "type": "boolean" }), options))
}

fn optional_session_affinity_format(options: Value) -> Property {
	opt(session_affinity_format(options))
}

static OPENAI_COMPLETIONS_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	properties([
		(
			"supportsStore",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider supports the store field. Default: auto-detected from URL.",
			})),
		),
		(
			"supportsDeveloperRole",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports the developer role instead of system. Default: auto-detected from URL.",
			})),
		),
		(
			"supportsReasoningEffort",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider supports reasoning_effort. Default: auto-detected from URL.",
			})),
		),
		(
			"supportsUsageInStreaming",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider supports stream_options.include_usage for token usage in streaming responses.",
				"default": true,
			})),
		),
		(
			"supportsFinishReason",
			opt(json!({
				"type": "boolean",
				"description": "Whether streamed responses include finish_reason. When false, pi infers stop or toolUse when the stream ends.",
				"default": true,
			})),
		),
		(
			"maxTokensField",
			opt(json!({
				"anyOf": [
					string_literal("max_completion_tokens", json!({})),
					string_literal("max_tokens", json!({})),
				],
				"description": "Which field to use for max tokens. Default: auto-detected from URL.",
			})),
		),
		(
			"requiresToolResultName",
			opt(json!({
				"type": "boolean",
				"description": "Whether tool results require the name field. Default: auto-detected from URL.",
			})),
		),
		(
			"requiresAssistantAfterToolResult",
			opt(json!({
				"type": "boolean",
				"description": "Whether a user message after tool results requires an assistant message in between. Default: auto-detected from URL.",
			})),
		),
		(
			"requiresThinkingAsText",
			opt(json!({
				"type": "boolean",
				"description": "Whether thinking blocks must be converted to text blocks with <thinking> delimiters. Default: auto-detected from URL.",
			})),
		),
		(
			"requiresReasoningContentOnAssistantMessages",
			opt(json!({
				"type": "boolean",
				"description": "Whether all replayed assistant messages must include an empty reasoning_content field when reasoning is enabled. Default: auto-detected from URL.",
			})),
		),
		(
			"thinkingFormat",
			opt(json!({
				"anyOf": [
					string_literal("openai", json!({})),
					string_literal("openrouter", json!({})),
					string_literal("deepseek", json!({})),
					string_literal("together", json!({})),
					string_literal("baseten", json!({})),
					string_literal("zai", json!({})),
					string_literal("qwen", json!({})),
					string_literal("chat-template", json!({})),
					string_literal("qwen-chat-template", json!({})),
					string_literal("string-thinking", json!({})),
					string_literal("ant-ling", json!({})),
				],
				"description": "Format for reasoning or thinking parameters. When omitted, Pi auto-detects the format from the provider URL. \"openai\" uses reasoning_effort, \"openrouter\" uses reasoning.effort, \"deepseek\" uses thinking.type plus reasoning_effort when supported, \"together\" uses reasoning.enabled plus reasoning_effort when supported, \"baseten\" uses configurable chat_template_args plus reasoning_effort when supported, \"zai\" uses thinking.type, \"qwen\" uses top-level enable_thinking, \"qwen-chat-template\" uses chat_template_kwargs.enable_thinking and preserve_thinking, \"chat-template\" uses configurable chat_template_kwargs, \"string-thinking\" uses top-level thinking, and \"ant-ling\" uses reasoning.effort only when the mapped effort is non-null.",
			})),
		),
		(
			"chatTemplateKwargs",
			opt(string_record(
				CHAT_TEMPLATE_KWARG_VALUE_SCHEMA.clone(),
				json!({
					"description": "Kwargs sent as chat_template_kwargs when thinkingFormat is \"chat-template\". Use $var with \"thinking.enabled\", \"thinking.effort\", or \"thinking.budget\" for pi-controlled values.",
				}),
			)),
		),
		(
			"chatTemplateArgs",
			opt(string_record(
				CHAT_TEMPLATE_KWARG_VALUE_SCHEMA.clone(),
				json!({
					"description": "Arguments sent as chat_template_args when thinkingFormat is \"baseten\". Use $var with \"thinking.enabled\", \"thinking.effort\", or \"thinking.budget\" for pi-controlled values.",
				}),
			)),
		),
		("openRouterRouting", opt(OPEN_ROUTER_ROUTING_SCHEMA.clone())),
		("vercelGatewayRouting", opt(VERCEL_GATEWAY_ROUTING_SCHEMA.clone())),
		(
			"zaiToolStream",
			opt(json!({
				"type": "boolean",
				"description": "Whether z.ai supports top-level tool_stream for streaming tool call deltas.",
				"default": false,
			})),
		),
		(
			"thinkingTokenBudgetField",
			opt(thinking_token_budget_field(json!({
				"description": "Top-level request field used to cap reasoning tokens from thinkingBudgets. Reasoning and the answer share max_tokens on these endpoints. \"thinking_token_budget\" is vLLM, \"thinking_budget\" is Qwen, DashScope, or SGLang, and \"thinking_budget_tokens\" is llama.cpp. Off by default and not set on the generated catalog.",
			}))),
		),
		(
			"supportsThinkingTokenBudget",
			opt(json!({
				"type": "boolean",
				"description": "Alias for thinkingTokenBudgetField: \"thinking_token_budget\" (vLLM). Prefer thinkingTokenBudgetField.",
				"default": false,
			})),
		),
		(
			"supportsOpenAIGrammarTools",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports OpenAI custom tools with Lark or regex grammar formats. When false, grammar-constrained tools fall back to normal function tools. The generated catalog enables this for capable models.",
				"default": false,
			})),
		),
		(
			"supportsMidConvoSystemMessages",
			optional_compat_boolean(json!({
				"description": "Whether the exact model accepts system or developer messages after the conversation has started. When false, later system messages are folded into the leading system message. The generated catalog enables this for verified models.",
				"default": false,
			})),
		),
		(
			"supportsMidConvoToolAdditions",
			opt(json!({
				"type": "boolean",
				"description": "Whether system messages can introduce additional tools mid-conversation. Requires supportsMidConvoSystemMessages. The generated catalog enables this for capable models.",
				"default": false,
			})),
		),
		(
			"supportsStrictMode",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports the strict field in tool definitions. Generated capable models enable it explicitly.",
				"default": false,
			})),
		),
		(
			"cacheControlFormat",
			opt(string_literal(
				"anthropic",
				json!({
					"description": "Cache control convention for prompt caching. Anthropic applies cache_control markers to the system prompt, last tool definition, and last user, assistant, or tool-result text content.",
				}),
			)),
		),
		(
			"sendSessionAffinityHeaders",
			optional_compat_boolean(json!({
				"description": "Whether to send session-affinity data from options.sessionId. Default: true for OpenRouter endpoints, false otherwise.",
			})),
		),
		(
			"sessionAffinityFormat",
			optional_session_affinity_format(json!({
				"description": "Session-affinity header format. openai sends session_id, x-client-request-id, and x-session-affinity; openai-nosession sends x-client-request-id and x-session-affinity; openrouter sends x-session-id. Does not affect prompt_cache_key. Default: auto-detected.",
			})),
		),
		(
			"supportsLongCacheRetention",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports long prompt cache retention (prompt_cache_retention: \"24h\" or Anthropic-style cache_control.ttl: \"1h\", depending on format). Default: auto-detected from provider and URL.",
			})),
		),
		(
			"vllmPriority",
			opt(json!({
				"type": "number",
				"description": "vLLM scheduler priority sent as the top-level priority request field. Lower values are handled earlier and the server default is 0. Only meaningful with --scheduling-policy priority. Off by default and not set on the generated catalog.",
			})),
		),
	])
});

pub static OPENAI_COMPLETIONS_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&OPENAI_COMPLETIONS_COMPAT_PROPERTIES,
		json!({
			"description": "Compatibility settings for OpenAI-compatible completions APIs. Use this to override URL-based auto-detection for custom providers.",
			"additionalProperties": true,
		}),
	)
});

static OPENAI_RESPONSES_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	properties([
		(
			"supportsDeveloperRole",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports the developer role instead of system.",
				"default": true,
			})),
		),
		(
			"supportsMidConvoSystemMessages",
			optional_compat_boolean(json!({
				"description": "Whether the exact model accepts developer or system messages after the conversation has started. When false, later system messages are folded into the leading system message. The generated catalog enables this for verified models.",
				"default": false,
			})),
		),
		(
			"sessionAffinityFormat",
			optional_session_affinity_format(json!({
				"description": "Session-affinity header format. openai sends session_id and x-client-request-id; openai-nosession sends x-client-request-id; openrouter sends x-session-id. Does not affect prompt_cache_key. Default: auto-detected.",
			})),
		),
		(
			"supportsLongCacheRetention",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports long prompt cache retention. This uses prompt_cache_options.ttl: \"30m\" on GPT-5.6+ and prompt_cache_retention: \"24h\" on earlier models.",
				"default": true,
			})),
		),
		(
			"supportsStrictMode",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports strict JSON-schema function tools. Defaults are API-specific; generated OpenAI models enable it explicitly.",
			})),
		),
		(
			"supportsOpenAIGrammarTools",
			optional_compat_boolean(json!({
				"description": "Whether to emit OpenAI custom tools with Lark or regex grammar formats. When false, grammar-constrained tools fall back to normal function tools. The generated catalog enables this for capable models.",
				"default": false,
			})),
		),
		(
			"supportsAdditionalTools",
			opt(json!({
				"type": "boolean",
				"description": "Whether the model supports message-anchored additional_tools input items.",
				"default": false,
			})),
		),
		(
			"supportsToolSearch",
			opt(json!({
				"type": "boolean",
				"description": "Whether the model supports client-executed tool search for transcript-anchored additions.",
				"default": false,
			})),
		),
		(
			"supportsExplicitPromptCacheMode",
			opt(json!({
				"type": "boolean",
				"description": "Whether the model accepts prompt_cache_options. Older OpenAI models reject the parameter.",
				"default": false,
			})),
		),
		(
			"supportsMidConvoEffort",
			opt(json!({
				"type": "boolean",
				"description": "Whether thinking level changes are sent as configuration_update input items instead of changing reasoning.effort, so a change keeps the prompt cache. Only used by the openai-responses API, and ignored with Sign in with ChatGPT on the openai provider.",
				"default": false,
			})),
		),
		(
			"supportsMaxOutputTokens",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider accepts max_output_tokens. Some Codex-protocol gateways reject it.",
				"default": true,
			})),
		),
	])
});

pub static OPENAI_RESPONSES_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&OPENAI_RESPONSES_COMPAT_PROPERTIES,
		json!({
			"description": "Compatibility settings for OpenAI Responses APIs.",
			"additionalProperties": true,
		}),
	)
});

static ANTHROPIC_MESSAGES_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	properties([
		(
			"supportsEagerToolInputStreaming",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider accepts per-tool eager_input_streaming. When false, the Anthropic provider omits tools[].eager_input_streaming and sends the legacy fine-grained-tool-streaming-2025-05-14 beta header for tool-enabled requests.",
				"default": true,
			})),
		),
		(
			"supportsLongCacheRetention",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports Anthropic long cache retention through cache_control.ttl.",
				"default": true,
			})),
		),
		(
			"sendSessionAffinityHeaders",
			optional_compat_boolean(json!({
				"description": "Whether to send x-session-affinity from options.sessionId when caching is enabled. Required for providers like Fireworks that use session affinity for prompt cache routing; requests to the same replica maximize cache hits. Default: true for OpenRouter endpoints, false otherwise.",
			})),
		),
		(
			"sessionAffinityFormat",
			opt(string_literal(
				"openrouter",
				json!({
					"description": "Session-affinity format. openrouter sends x-session-id; when unset, sends x-session-affinity.",
				}),
			)),
		),
		(
			"supportsCacheControlOnTools",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider supports Anthropic-style cache_control markers on tool definitions. When false, cache_control is omitted from tool parameters. Some Anthropic-compatible providers, such as Fireworks, do not support this field on tools and may reject or ignore it.",
				"default": true,
			})),
		),
		(
			"supportsTemperature",
			opt(json!({
				"type": "boolean",
				"description": "Whether the model accepts the Anthropic temperature request field. Claude Opus 4.7+ rejects non-default values.",
				"default": true,
			})),
		),
		(
			"forceAdaptiveThinking",
			opt(json!({
				"type": "boolean",
				"description": "Whether to force adaptive thinking (thinking.type: adaptive plus output_config.effort) regardless of model ID. Built-in models that require adaptive thinking set this in generated metadata. Custom Anthropic-compatible providers can set this to true for any model whose upstream requires the adaptive format. Set false to opt out on overridden built-in models.",
				"default": false,
			})),
		),
		(
			"allowEmptySignature",
			opt(json!({
				"type": "boolean",
				"description": "Whether to replay empty thinking signatures instead of converting thinking to text.",
				"default": false,
			})),
		),
		(
			"supportsStrictTools",
			opt(json!({
				"type": "boolean",
				"description": "Whether the provider supports Anthropic strict tool schemas. Generated Anthropic models enable it explicitly.",
				"default": false,
			})),
		),
		(
			"supportsMidConvoEffort",
			opt(json!({
				"type": "boolean",
				"description": "Whether the exact model transport supports effort-only system messages and thinking binding controls.",
				"default": false,
			})),
		),
		(
			"supportsMidConvoSystemMessages",
			optional_compat_boolean(json!({
				"description": "Whether the exact model accepts system-role messages inside the conversation. When false, later system messages are folded into the top-level system prompt.",
				"default": false,
			})),
		),
		(
			"supportsMidConvoToolChanges",
			opt(json!({
				"type": "boolean",
				"description": "Whether the exact model accepts mid-conversation tool_addition and tool_removal blocks. Requires supportsMidConvoSystemMessages.",
				"default": false,
			})),
		),
		(
			"allowedFallbackModels",
			opt(json!({
				"type": "array",
				"items": ANTHROPIC_ALLOWED_FALLBACK_MODEL_SCHEMA.clone(),
				"maxItems": 3,
				"description": "Models Anthropic accepts for server-side refusal fallback, with local pricing metadata for returned fallback responses. When absent or empty, callers must omit fallbacks; Anthropic rejects the field for models with no permitted fallback targets.",
			})),
		),
	])
});

pub static ANTHROPIC_MESSAGES_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&ANTHROPIC_MESSAGES_COMPAT_PROPERTIES,
		json!({
			"description": "Compatibility settings for Anthropic Messages-compatible APIs.",
			"additionalProperties": true,
		}),
	)
});

static BEDROCK_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	properties([(
		"supportsStrictMode",
		optional_compat_boolean(json!({
			"description": "Whether the model supports Bedrock strict tool schemas.",
			"default": false,
		})),
	)])
});

pub static BEDROCK_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&BEDROCK_COMPAT_PROPERTIES,
		json!({
			"description": "Compatibility settings for Amazon Bedrock models.",
			"additionalProperties": true,
		}),
	)
});

static MISTRAL_CONVERSATIONS_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	properties([(
		"supportsMidConvoSystemMessages",
		optional_compat_boolean(json!({
			"description": "Whether the exact model accepts system messages after the conversation has started. When false, later system messages are folded into the leading system message.",
			"default": false,
		})),
	)])
});

pub static MISTRAL_CONVERSATIONS_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&MISTRAL_CONVERSATIONS_COMPAT_PROPERTIES,
		json!({
			"description": "Compatibility settings for the Mistral chat API.",
			"additionalProperties": true,
		}),
	)
});

static SYSTEM_ONE_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	properties([
		(
			"imageInput",
			opt(json!({
				"anyOf": [
					string_literal("assets", json!({})),
					string_literal("images", json!({})),
					string_literal("state-image-urls", json!({})),
				],
				"description": "How the service takes classifier images. \"assets\" sends TypeSafe's assets map (PNG and JPEG) and references the images from the state, as TypeSafe and Jev on Workers AI expect. \"images\" sends Clef's images list of data URLs (PNG, JPEG, and WebP, at most four), as Clef on Workers AI expects. \"state-image-urls\" places image_url parts before the state, as OpenRouter expects; OpenRouter drops assets without an error. Auto-detected from the provider, baseUrl, and model ID. Classifier models read it from the provider-level compat; models and modelOverrides entries apply to chat models only.",
			})),
		),
		(
			"sendImageOptInHeader",
			optional_compat_boolean(json!({
				"description": "Whether requests with images send the TypeSafe-Danger-Zone-Images: true header, which TypeSafe's own API requires for its experimental image input. Defaults to true for the typesafe-system-one API unless the model is detected as OpenRouter.",
			})),
		),
	])
});

pub static SYSTEM_ONE_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&SYSTEM_ONE_COMPAT_PROPERTIES,
		json!({
			"description": "Compatibility settings for System One classifier APIs.",
			"additionalProperties": true,
		}),
	)
});

static PROVIDER_COMPAT_PROPERTY_OVERRIDES: LazyLock<Properties> = LazyLock::new(|| {
	properties([
		(
			"supportsDeveloperRole",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports the developer role instead of system. Defaults are API-specific.",
			})),
		),
		(
			"supportsMidConvoSystemMessages",
			optional_compat_boolean(json!({
				"description": "Whether the exact model accepts system or developer messages after the conversation has started. When false, later system messages are folded into the leading system message.",
				"default": false,
			})),
		),
		(
			"sessionAffinityFormat",
			optional_session_affinity_format(json!({
				"description": "Session-affinity header format. Defaults are API-specific or auto-detected.",
			})),
		),
		(
			"supportsMidConvoEffort",
			optional_compat_boolean(json!({
				"description": "Whether thinking level changes are sent inside the conversation instead of changing the request's effort, so a change keeps the prompt cache. Used by the anthropic-messages and openai-responses APIs.",
				"default": false,
			})),
		),
		(
			"supportsLongCacheRetention",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports long prompt cache retention. Defaults are API-specific or auto-detected.",
			})),
		),
		(
			"supportsStrictMode",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports strict tool schemas. Defaults are API-specific.",
			})),
		),
		(
			"supportsOpenAIGrammarTools",
			optional_compat_boolean(json!({
				"description": "Whether the provider supports OpenAI custom tools with Lark or regex grammar formats. When false, grammar-constrained tools fall back to normal function tools.",
				"default": false,
			})),
		),
		(
			"sendSessionAffinityHeaders",
			optional_compat_boolean(json!({
				"description": "Whether to send session-affinity data from options.sessionId. Defaults are API-specific.",
			})),
		),
	])
});

/// Provider-level models.json compatibility is API-agnostic, so it needs one superset. Every
/// duplicate property requires explicit generic metadata, while API-specific schemas retain their
/// own defaults and descriptions.
fn merge_compat_properties(property_groups: &[&Properties], overrides: &Properties) -> PiResult<Properties> {
	let mut merged = Properties::new();
	let mut duplicate_names = IndexSet::new();

	for properties in property_groups {
		for (name, schema) in properties.iter() {
			if merged.contains_key(name) {
				duplicate_names.insert(name.clone());
				if !overrides.contains_key(name) {
					return Err(PiError::new(format!(
						"Duplicate compatibility schema property requires an override: {name}"
					)));
				}
			}
			merged.insert(name.clone(), schema.clone());
		}
	}

	for name in overrides.keys() {
		if !duplicate_names.contains(name) {
			return Err(PiError::new(format!(
				"Compatibility schema override does not resolve a duplicate property: {name}"
			)));
		}
	}

	for (name, schema) in overrides {
		merged.insert(name.clone(), schema.clone());
	}
	Ok(merged)
}

/// Compatibility schema for each API whose models accept compat. Mirrors the compat type of Model.
pub static COMPAT_SCHEMAS_BY_API: LazyLock<IndexMap<&'static str, &'static Value>> = LazyLock::new(|| {
	IndexMap::from([
		("openai-completions", &*OPENAI_COMPLETIONS_COMPAT_SCHEMA),
		("openai-responses", &*OPENAI_RESPONSES_COMPAT_SCHEMA),
		("azure-openai-responses", &*OPENAI_RESPONSES_COMPAT_SCHEMA),
		("openai-codex-responses", &*OPENAI_RESPONSES_COMPAT_SCHEMA),
		("anthropic-messages", &*ANTHROPIC_MESSAGES_COMPAT_SCHEMA),
		("bedrock-converse-stream", &*BEDROCK_COMPAT_SCHEMA),
		("mistral-conversations", &*MISTRAL_CONVERSATIONS_COMPAT_SCHEMA),
		("typesafe-system-one", &*SYSTEM_ONE_COMPAT_SCHEMA),
		("cloudflare-workers-ai-system-one", &*SYSTEM_ONE_COMPAT_SCHEMA),
	])
});

static PROVIDER_COMPAT_PROPERTIES: LazyLock<Properties> = LazyLock::new(|| {
	merge_compat_properties(
		&[
			&OPENAI_COMPLETIONS_COMPAT_PROPERTIES,
			&OPENAI_RESPONSES_COMPAT_PROPERTIES,
			&ANTHROPIC_MESSAGES_COMPAT_PROPERTIES,
			&BEDROCK_COMPAT_PROPERTIES,
			&MISTRAL_CONVERSATIONS_COMPAT_PROPERTIES,
			&SYSTEM_ONE_COMPAT_PROPERTIES,
		],
		&PROVIDER_COMPAT_PROPERTY_OVERRIDES,
	)
	.expect("compatibility schema properties merge: every duplicate has an override")
});

pub static PROVIDER_COMPAT_SCHEMA: LazyLock<Value> = LazyLock::new(|| {
	open_object(
		&PROVIDER_COMPAT_PROPERTIES,
		json!({
			"description": "Provider and model compatibility overrides.",
			"additionalProperties": true,
		}),
	)
});

fn properties<const N: usize>(entries: [(&str, Property); N]) -> Properties {
	entries
		.into_iter()
		.map(|(name, property)| (name.to_string(), property))
		.collect()
}

fn with_options(schema: Value, options: Value) -> Value {
	let (Value::Object(mut schema), Value::Object(options)) = (schema, options) else {
		unreachable!("schema builders take object schemas and options");
	};
	schema.extend(options);
	Value::Object(schema)
}

/// `Static<typeof ChatTemplateKwargValueSchema>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatTemplateKwargValue {
	String(String),
	Number(f64),
	Bool(bool),
	Null,
	Var(ChatTemplateVar),
}

/// A pi-controlled chat template value: `{ $var, omitWhenOff? }`. Kept as the configured JSON object or array: TS
/// resolves every non-null object as a variable, whatever its keys, so one the schema would reject still reaches the
/// request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChatTemplateVar(pub Value);

impl ChatTemplateVar {
	/// `value.$var` when it names a known variable. Any other value resolves like `thinking.effort`.
	pub fn var(&self) -> Option<ChatTemplateVarName> {
		self.0
			.get("$var")
			.and_then(|name| ChatTemplateVarName::deserialize(name).ok())
	}

	/// Whether `value.omitWhenOff` is truthy.
	pub fn omit_when_off(&self) -> bool {
		match self.0.get("omitWhenOff") {
			None | Some(Value::Null) => false,
			Some(Value::Bool(value)) => *value,
			Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
			Some(Value::String(value)) => !value.is_empty(),
			Some(Value::Array(_) | Value::Object(_)) => true,
		}
	}
}

/// The `$var` names of a chat template value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChatTemplateVarName {
	#[serde(rename = "thinking.enabled")]
	ThinkingEnabled,
	#[serde(rename = "thinking.effort")]
	ThinkingEffort,
	#[serde(rename = "thinking.budget")]
	ThinkingBudget,
}

/// Top-level request field used to cap reasoning tokens on OpenAI-compatible servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThinkingTokenBudgetField {
	#[serde(rename = "thinking_token_budget")]
	ThinkingTokenBudget,
	#[serde(rename = "thinking_budget")]
	ThinkingBudget,
	#[serde(rename = "thinking_budget_tokens")]
	ThinkingBudgetTokens,
}

/// `Static<typeof SessionAffinityFormatSchema>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionAffinityFormat {
	#[serde(rename = "openai")]
	Openai,
	#[serde(rename = "openai-nosession")]
	OpenaiNosession,
	#[serde(rename = "openrouter")]
	Openrouter,
}

/// Which request field carries max tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaxTokensField {
	#[serde(rename = "max_completion_tokens")]
	MaxCompletionTokens,
	#[serde(rename = "max_tokens")]
	MaxTokens,
}

/// Format for reasoning or thinking parameters of OpenAI-compatible completions APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThinkingFormat {
	#[serde(rename = "openai")]
	Openai,
	#[serde(rename = "openrouter")]
	Openrouter,
	#[serde(rename = "deepseek")]
	Deepseek,
	#[serde(rename = "together")]
	Together,
	#[serde(rename = "baseten")]
	Baseten,
	#[serde(rename = "zai")]
	Zai,
	#[serde(rename = "qwen")]
	Qwen,
	#[serde(rename = "chat-template")]
	ChatTemplate,
	#[serde(rename = "qwen-chat-template")]
	QwenChatTemplate,
	#[serde(rename = "string-thinking")]
	StringThinking,
	#[serde(rename = "ant-ling")]
	AntLing,
}

/// Cache control convention for prompt caching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CacheControlFormat {
	#[serde(rename = "anthropic")]
	Anthropic,
}

/// Session-affinity format of Anthropic Messages-compatible APIs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AnthropicSessionAffinityFormat {
	#[serde(rename = "openrouter")]
	Openrouter,
}

/// How a System One service takes classifier images.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SystemOneImageInput {
	#[serde(rename = "assets")]
	Assets,
	#[serde(rename = "images")]
	Images,
	#[serde(rename = "state-image-urls")]
	StateImageUrls,
}

/// Provider and model compatibility overrides in models.json: every API's compat fields.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_store: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_developer_role: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_reasoning_effort: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_usage_in_streaming: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_finish_reason: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_tokens_field: Option<MaxTokensField>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_tool_result_name: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_assistant_after_tool_result: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_thinking_as_text: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_reasoning_content_on_assistant_messages: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_format: Option<ThinkingFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub chat_template_kwargs: Option<IndexMap<String, ChatTemplateKwargValue>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub chat_template_args: Option<IndexMap<String, ChatTemplateKwargValue>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub open_router_routing: Option<OpenRouterRouting>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vercel_gateway_routing: Option<VercelGatewayRouting>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zai_tool_stream: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_thinking_token_budget: Option<bool>,
	#[serde(
		rename = "supportsOpenAIGrammarTools",
		default,
		skip_serializing_if = "Option::is_none"
	)]
	pub supports_openai_grammar_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_system_messages: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_tool_additions: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_strict_mode: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cache_control_format: Option<CacheControlFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub send_session_affinity_headers: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_affinity_format: Option<SessionAffinityFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_long_cache_retention: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vllm_priority: Option<f64>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_additional_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_tool_search: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_explicit_prompt_cache_mode: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_effort: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_max_output_tokens: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_eager_tool_input_streaming: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_cache_control_on_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_temperature: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub force_adaptive_thinking: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_empty_signature: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_strict_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_tool_changes: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allowed_fallback_models: Option<Vec<AnthropicAllowedFallbackModel>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub image_input: Option<SystemOneImageInput>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub send_image_opt_in_header: Option<bool>,
}

/// OpenRouter provider routing preferences. Sent verbatim as the request's `provider` field, so the object keeps its
/// keys and their order; see `OPEN_ROUTER_ROUTING_SCHEMA` for the known keys.
pub type OpenRouterRouting = Map<String, Value>;

/// Vercel AI Gateway routing preferences, kept as the configured JSON object; see `VERCEL_GATEWAY_ROUTING_SCHEMA`.
pub type VercelGatewayRouting = Map<String, Value>;

/// An Anthropic server-side refusal fallback model with local pricing metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicAllowedFallbackModel {
	pub provider: String,
	pub model: String,
	pub cost: ModelCost,
}

/// Compatibility settings for OpenAI-compatible completions APIs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAICompletionsCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_store: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_developer_role: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_reasoning_effort: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_usage_in_streaming: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_finish_reason: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub max_tokens_field: Option<MaxTokensField>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_tool_result_name: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_assistant_after_tool_result: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_thinking_as_text: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub requires_reasoning_content_on_assistant_messages: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_format: Option<ThinkingFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub chat_template_kwargs: Option<IndexMap<String, ChatTemplateKwargValue>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub chat_template_args: Option<IndexMap<String, ChatTemplateKwargValue>>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub open_router_routing: Option<OpenRouterRouting>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vercel_gateway_routing: Option<VercelGatewayRouting>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub zai_tool_stream: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub thinking_token_budget_field: Option<ThinkingTokenBudgetField>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_thinking_token_budget: Option<bool>,
	#[serde(
		rename = "supportsOpenAIGrammarTools",
		default,
		skip_serializing_if = "Option::is_none"
	)]
	pub supports_openai_grammar_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_system_messages: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_tool_additions: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_strict_mode: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cache_control_format: Option<CacheControlFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub send_session_affinity_headers: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_affinity_format: Option<SessionAffinityFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_long_cache_retention: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub vllm_priority: Option<f64>,
}

/// Compatibility settings for OpenAI Responses APIs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAIResponsesCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_developer_role: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_system_messages: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_affinity_format: Option<SessionAffinityFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_long_cache_retention: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_strict_mode: Option<bool>,
	#[serde(
		rename = "supportsOpenAIGrammarTools",
		default,
		skip_serializing_if = "Option::is_none"
	)]
	pub supports_openai_grammar_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_additional_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_tool_search: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_explicit_prompt_cache_mode: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_effort: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_max_output_tokens: Option<bool>,
}

/// Compatibility settings for Anthropic Messages-compatible APIs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnthropicMessagesCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_eager_tool_input_streaming: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_long_cache_retention: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub send_session_affinity_headers: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub session_affinity_format: Option<AnthropicSessionAffinityFormat>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_cache_control_on_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_temperature: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub force_adaptive_thinking: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allow_empty_signature: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_strict_tools: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_effort: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_system_messages: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_tool_changes: Option<bool>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub allowed_fallback_models: Option<Vec<AnthropicAllowedFallbackModel>>,
}

/// Compatibility settings for Amazon Bedrock models.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BedrockCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_strict_mode: Option<bool>,
}

/// Compatibility settings for the Mistral chat API.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MistralConversationsCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub supports_mid_convo_system_messages: Option<bool>,
}

/// Compatibility settings for System One classifier APIs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemOneCompat {
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub image_input: Option<SystemOneImageInput>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub send_image_opt_in_header: Option<bool>,
}

//! Port of `packages/ai/test/overflow.test.ts`.
//!
//! `it.each` tables are loops inside one test named after the table. The backtracking test runs the patterns in
//! process instead of in a child Node process with a timeout; the Rust regex engine is linear for these patterns.

use pi_ai::types::{AssistantMessage, StopReason, Usage};
use pi_ai::utils::overflow::{is_context_overflow, is_recoverable_length};

fn create_error_message(error_message: &str, provider: &str) -> AssistantMessage {
	AssistantMessage {
		content: vec![],
		api: "openai-completions".into(),
		provider: provider.into(),
		model: "qwen3.5:35b".into(),
		usage: Usage::default(),
		stop_reason: StopReason::Error,
		error_message: Some(error_message.into()),
		timestamp: 1_700_000_000_000.0,
		..AssistantMessage::default()
	}
}

fn error_message(error_message: &str) -> AssistantMessage {
	create_error_message(error_message, "ollama")
}

struct LengthStop {
	input: f64,
	cache_read: f64,
	output: f64,
	cache_write: f64,
	api: &'static str,
	provider: &'static str,
	model: &'static str,
}

impl Default for LengthStop {
	fn default() -> Self {
		Self {
			input: 0.0,
			cache_read: 0.0,
			output: 0.0,
			cache_write: 0.0,
			api: "openai-completions",
			provider: "test-provider",
			model: "test-model",
		}
	}
}

fn create_length_stop_message(options: LengthStop) -> AssistantMessage {
	AssistantMessage {
		content: vec![],
		api: options.api.into(),
		provider: options.provider.into(),
		model: options.model.into(),
		usage: Usage {
			input: options.input,
			output: options.output,
			cache_read: options.cache_read,
			cache_write: options.cache_write,
			total_tokens: options.input + options.cache_read + options.cache_write + options.output,
			..Usage::default()
		},
		stop_reason: StopReason::Length,
		timestamp: 1_700_000_000_000.0,
		..AssistantMessage::default()
	}
}

mod is_context_overflow {
	use super::*;

	#[test]
	fn detects_provider_independent_request_size_overflow() {
		// The 413 pattern is provider-agnostic, so a single Copilot-prefixed string and a single
		// non-Copilot string cover both branches of the regex; the bare forms cover the raw SDK output.
		for message in [
			r#"github-copilot API error (413): {"message":"failed to parse request","code":""}"#, // formatProviderError prefix
			"custom-provider API error (413): Payload Too Large", // any provider, not just Copilot
			r#"413 {"error":{"message":"failed to parse request","code":""}}"#, // raw SDK error
			"413",                                                // bare status at end of string
		] {
			// The request can exceed the byte limit even with zero usage and a large token window.
			assert!(
				is_context_overflow(&error_message(message), Some(1_000_000.0)),
				"{message}"
			);
			assert!(is_context_overflow(&error_message(message), None), "{message}");
		}
	}

	#[test]
	fn does_not_mistake_unrelated_errors_for_request_size_overflow() {
		for message in [
			r#"github-copilot API error (400): {"message":"failed to parse request","code":""}"#, // other status, same body
			"custom-provider API error (429): Too many requests",
			r#"github-copilot API error (400): {"message":"failed to parse request: invalid value 413"}"#, // 413 in the body only
			"custom-provider API error (400): upstream mentioned OpenAI API error (413): invalid input",   // 413 not at start
			"custom-provider API error (4130): invalid input", // 413 is a prefix, not the status
			"4130 failed to parse request",                    // bare 4130 is not 413
		] {
			assert!(!is_context_overflow(&error_message(message), None), "{message}");
		}
	}

	#[test]
	fn limits_http_status_matching_to_failed_responses() {
		let message = error_message(r#"OpenAI API error (413): {"message":"failed to parse request"}"#);
		assert!(!is_context_overflow(
			&AssistantMessage {
				stop_reason: StopReason::Aborted,
				..message.clone()
			},
			None
		));
		assert!(!is_context_overflow(
			&AssistantMessage {
				stop_reason: StopReason::Stop,
				..message
			},
			None
		));
	}

	#[test]
	fn detects_explicit_ollama_prompt_too_long_errors() {
		let message = error_message("400 `prompt too long; exceeded max context length by 100918 tokens`");
		assert!(is_context_overflow(&message, Some(32768.0)));
	}

	#[test]
	fn detects_z_ai_prompt_too_long_errors() {
		// Regression for #9805.
		let message = create_error_message(r#"400 {"code":"1261","message":"Prompt too long"}"#, "zai");
		assert!(is_context_overflow(&message, Some(1048576.0)));
	}

	#[test]
	fn detects_z_ai_cn_endpoint_prompt_exceeds_max_length_errors() {
		// Regression for #10208.
		let message = create_error_message(r#"400 {"code":"1261","message":"Prompt exceeds max length"}"#, "zai");
		assert!(is_context_overflow(&message, Some(1048576.0)));
	}

	#[test]
	fn detects_together_ai_context_length_errors() {
		let message =
			error_message("400 The input (516368 tokens) is longer than the model's context length (262144 tokens).");
		assert!(is_context_overflow(&message, Some(262144.0)));
	}

	#[test]
	fn detects_litellm_wrapped_openai_maximum_context_length_errors() {
		let message = error_message(
			"Error: 503 litellm.ServiceUnavailableError: litellm.MidStreamFallbackError: litellm.APIConnectionError: APIConnectionError: OpenAIException - Requested token count exceeds the model's maximum context length of 131072 tokens.",
		);
		assert!(is_context_overflow(&message, Some(131072.0)));
	}

	#[test]
	fn detects_litellm_pre_call_context_window_exceeded_error() {
		// Regression for #11492.
		let message = error_message(
			"400: {\"message\":\"litellm.ContextWindowExceededError: litellm.BadRequestError: litellm._pre_call_checks: Context Window exceeded for given call. No models have context window large enough for this call.\nModel=Qwen3.8-Flash-Next, Max Input Tokens=262144, Got=265262\nmodel=Qwen3.8-Flash-Next. context_window_fallbacks=None. fallbacks=[].\"}",
		);
		assert!(is_context_overflow(&message, Some(262144.0)));
	}

	#[test]
	fn detects_openai_compatible_parenthesized_maximum_context_length_errors() {
		let message =
			error_message("Error: 400 Input length (265330) exceeds model's maximum context length (262144).");
		assert!(is_context_overflow(&message, Some(262144.0)));
	}

	#[test]
	fn detects_openrouter_poolside_maximum_allowed_input_length_errors() {
		let message = error_message(
			"Provider returned error: Input length 131393 exceeds the maximum allowed input length of 131040 tokens.",
		);
		assert!(is_context_overflow(&message, Some(131072.0)));
	}

	#[test]
	fn detects_ds4_configured_context_size_errors() {
		let message = error_message("400 Prompt has 256468 tokens, but the configured context size is 256000 tokens");
		assert!(is_context_overflow(&message, Some(256000.0)));

		let comma_message =
			error_message("Prompt has 5,958,968 tokens, but the configured context size is 256,000 tokens");
		assert!(is_context_overflow(&comma_message, Some(256000.0)));
	}

	#[test]
	fn does_not_treat_generic_non_overflow_ollama_errors_as_overflow() {
		let message = error_message("500 `model runner crashed unexpectedly`");
		assert!(!is_context_overflow(&message, Some(32768.0)));
	}

	#[test]
	fn only_treats_bodyless_400_errors_as_overflow_for_cerebras() {
		// Regression for #9482. HTTP 413 is a request-size error for every provider,
		// but a bodyless HTTP 400 is only known to indicate overflow for Cerebras.
		let error_message = "400 status code (no body)";
		assert!(is_context_overflow(
			&create_error_message(error_message, "cerebras"),
			Some(131072.0)
		));
		assert!(!is_context_overflow(
			&create_error_message(error_message, "opencode-go"),
			Some(1000000.0)
		));
	}

	#[test]
	fn accepts_cerebras_bodyless_errors_with_and_without_the_optional_status_label() {
		for status in ["400", "413"] {
			for separator in ["", " ", "\t", "\r\n"] {
				for label in ["", "status code", "STATUS CODE"] {
					let message =
						create_error_message(&format!("{status}{separator}{label}{separator}(no body)"), "cerebras");
					assert!(is_context_overflow(&message, None), "{:?}", message.error_message);
				}
			}
		}
	}

	#[test]
	fn rejects_long_whitespace_only_cerebras_errors_without_backtracking() {
		let mut message = create_error_message("", "cerebras");
		let whitespace = "\t".repeat(100_000);
		for suffix in ["", "x", "(no bodx)"] {
			message.error_message = Some(format!("400{whitespace}{suffix}"));
			assert!(!is_context_overflow(&message, None));
		}
		message.error_message = Some(format!("413{whitespace}(no body)"));
		assert!(is_context_overflow(&message, None));
	}

	#[test]
	fn does_not_treat_bedrock_throttling_too_many_tokens_as_overflow() {
		// Bedrock returns this for HTTP 429 rate limiting, NOT context overflow.
		// formatBedrockError uses a human-readable prefix for ThrottlingException.
		let message = error_message("Throttling error: Too many tokens, please wait before trying again.");
		assert!(!is_context_overflow(&message, Some(200000.0)));
	}

	#[test]
	fn does_not_treat_bedrock_service_unavailable_as_overflow() {
		let message = error_message("Service unavailable: The service is temporarily unavailable.");
		assert!(!is_context_overflow(&message, Some(200000.0)));
	}

	#[test]
	fn does_not_treat_generic_rate_limit_errors_as_overflow() {
		let message = error_message("Rate limit exceeded, please retry after 30 seconds.");
		assert!(!is_context_overflow(&message, Some(200000.0)));
	}

	#[test]
	fn does_not_treat_http_429_style_errors_as_overflow() {
		let message = error_message("Too many requests. Please slow down.");
		assert!(!is_context_overflow(&message, Some(200000.0)));
	}

	#[test]
	fn detects_xiaomi_style_overflow_length_stop_with_zero_output_and_filled_context() {
		let message = create_length_stop_message(LengthStop {
			input: 58.0,
			cache_read: 1048512.0,
			output: 0.0,
			provider: "xiaomi",
			model: "mimo-v2.6-pro",
			..LengthStop::default()
		});
		assert!(is_context_overflow(&message, Some(1048576.0)));
	}

	#[test]
	fn treats_a_length_stop_below_the_desired_output_limit_as_recoverable() {
		let message = create_length_stop_message(LengthStop {
			input: 3.0,
			cache_read: 253584.0,
			cache_write: 25554.0,
			output: 16.0,
			api: "openai-responses",
			provider: "openai",
			model: "gpt-5.6-sol",
		});
		assert!(is_recoverable_length(&message, 128000.0));
	}

	#[test]
	fn does_not_recover_a_length_stop_that_reached_the_desired_output_limit() {
		let message = create_length_stop_message(LengthStop {
			input: 4062.0,
			cache_read: 0.0,
			output: 1024.0,
			..LengthStop::default()
		});
		assert!(!is_recoverable_length(&message, 1024.0));
	}

	#[test]
	fn treats_zero_output_length_stops_as_recoverable_without_context_metadata() {
		let message = create_length_stop_message(LengthStop {
			input: 100.0,
			cache_read: 0.0,
			output: 0.0,
			..LengthStop::default()
		});
		assert!(is_recoverable_length(&message, 128000.0));
	}

	#[test]
	fn does_not_treat_normal_length_stops_with_output_as_context_overflow() {
		let message = create_length_stop_message(LengthStop {
			input: 1000.0,
			cache_read: 0.0,
			output: 4096.0,
			..LengthStop::default()
		});
		assert!(!is_context_overflow(&message, Some(200000.0)));
	}

	#[test]
	fn does_not_treat_zero_output_length_stops_far_below_context_as_context_overflow() {
		let message = create_length_stop_message(LengthStop {
			input: 100.0,
			cache_read: 0.0,
			output: 0.0,
			..LengthStop::default()
		});
		assert!(!is_context_overflow(&message, Some(200000.0)));
	}
}

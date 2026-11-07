//! Port of `packages/ai/test/openai-completions-raw-stop-reason.test.ts`.

mod openai_completions_support;

use openai_completions_support::{context, model, options, serve_chunks, user_message};
use pi_ai::api::openai_completions::stream;
use pi_ai::types::StopReason;
use pi_host::testing::run;
use serde_json::json;

mod openai_completions_raw_stop_reasons {
	use super::*;

	fn test_model() -> pi_ai::types::Model {
		model("test-model", "openai", "https://api.openai.com/v1")
	}

	#[test]
	fn preserves_raw_finish_reasons_for_successful_stops() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![json!({ "id": "chatcmpl-1", "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] })],
			);

			let message = stream(
				&test_model(),
				&context(vec![user_message("hello")]),
				Some(options("test")),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(message.stop_reason, StopReason::Stop);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("stop"));
			assert_eq!(message.error_message, None);
		});
	}

	#[test]
	fn preserves_raw_finish_reasons_for_provider_error_stops() {
		run(|host| async move {
			serve_chunks(
				&host,
				vec![
					json!({ "id": "chatcmpl-2", "choices": [{ "index": 0, "delta": {}, "finish_reason": "content_filter" }] }),
				],
			);

			let message = stream(
				&test_model(),
				&context(vec![user_message("hello")]),
				Some(options("test")),
			)
			.result()
			.await
			.unwrap();

			assert_eq!(message.stop_reason, StopReason::Error);
			assert_eq!(message.raw_stop_reason.as_deref(), Some("content_filter"));
			assert_eq!(
				message.error_message.as_deref(),
				Some("Provider finish_reason: content_filter")
			);
		});
	}
}

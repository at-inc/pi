//! Port of `packages/ai/test/bedrock-custom-headers.test.ts`.
//!
//! The TS `vi.mock`s `@aws-sdk/client-bedrock-runtime`, records the `middlewareStack.add` registrations and calls the
//! registered `pi-ai-custom-headers` handler on fake `args.request.headers`. The Rust tests answer at the HTTP level
//! and read the headers the client sent, so:
//! - "registers a build-step middleware" becomes the caller header on the request and in the SigV4 `SignedHeaders`
//!   (a `build`-step header is set before signing). The registration's `priority: "low"` and `name` have no wire
//!   effect and are left out; `nextSpy` called once is one recorded request.
//! - "skips reserved headers" sends the reserved headers on a SigV4 request and checks that the client's own
//!   `authorization`, `x-amz-date` and `host` win. The recorded request has no `host` header (the client drops it
//!   for the host's fetch, like the SDK's HTTP/2 handler), so a leaked `HOST` would show as an extra header name.
//! - "registers no middleware" becomes a request with exactly the client's own headers.
//! - "VC3 (structural guard): passes through unchanged when the request has no headers" is left out: it calls the
//!   handler with `{ request: {} }` and `{ request: undefined }`, which the Rust build middleware cannot receive (it
//!   always gets the request's header map), and nothing at the HTTP level can show it.
//! - The TS process env has no AWS keys under the mock; the Rust tests set ambient
//!   `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` so the request is SigV4-signed, and serve a successful response
//!   where the mocked `send()` rejects (`driveBedrock` ignores the result either way).

mod bedrock_support;

use bedrock_support::{serve, stream_response, text_stream};
use pi_ai::api::bedrock_converse_stream::{BedrockOptions, stream, stream_simple};
use pi_ai::types::{
	CacheRetention, Model, ProviderHeaders, ProviderRequestOptions, SimpleStreamOptions, StreamOptions,
	TranscriptContext,
};
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::json;

fn context() -> TranscriptContext {
	bedrock_support::context(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] }))
}

/// `getModelFixture()`.
fn get_model_fixture() -> Model {
	bedrock_support::builtin_model("us.anthropic.claude-opus-4-8")
}

/// The client's own header names on a SigV4 request.
const CLIENT_HEADERS: [&str; 9] = [
	"content-type",
	"content-length",
	"x-amz-user-agent",
	"user-agent",
	"amz-sdk-invocation-id",
	"amz-sdk-request",
	"x-amz-date",
	"x-amz-content-sha256",
	"authorization",
];

fn headers(entries: &[(&str, &str)]) -> ProviderHeaders {
	entries
		.iter()
		.map(|(key, value)| (key.to_string(), Some(value.to_string())))
		.collect()
}

/// `{ cacheRetention: "none", headers }`.
fn options(headers: Option<ProviderHeaders>) -> BedrockOptions {
	BedrockOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				headers,
				..ProviderRequestOptions::default()
			},
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		..BedrockOptions::default()
	}
}

/// Ambient access keys, so the request is SigV4-signed, and a model response.
fn setup(host: &TestHost) {
	host.set_env("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
	host.set_env("AWS_SECRET_ACCESS_KEY", "secretexample");
	serve(host, vec![stream_response(&text_stream("ok"), &[])]);
}

/// `driveBedrock(options)`: the one request the stream sent.
async fn drive_bedrock(host: &TestHost, options: BedrockOptions) -> RecordedRequest {
	let _ = stream(&get_model_fixture(), &context(), Some(options)).result().await;
	only_request(host)
}

fn only_request(host: &TestHost) -> RecordedRequest {
	let requests = host.requests();
	let [request] = &requests[..] else {
		panic!("expected one request, got {requests:?}");
	};
	request.clone()
}

/// The request's header names, sorted.
fn sorted_names(request: &RecordedRequest) -> Vec<String> {
	let mut names: Vec<String> = request.headers.keys().cloned().collect();
	names.sort();
	names
}

/// `CLIENT_HEADERS` plus `extra`, sorted.
fn expected_names(extra: &[&str]) -> Vec<String> {
	let mut names: Vec<String> = CLIENT_HEADERS
		.iter()
		.chain(extra)
		.map(|name| name.to_string())
		.collect();
	names.sort();
	names
}

/// The `SignedHeaders` of the SigV4 `authorization` header.
fn signed_headers(request: &RecordedRequest) -> Vec<String> {
	let authorization = request.header("authorization").unwrap_or_default();
	let signed = authorization
		.split(", ")
		.find_map(|part| part.strip_prefix("SignedHeaders="))
		.unwrap_or_else(|| panic!("expected a SigV4 authorization header, got {authorization:?}"));
	signed.split(';').map(str::to_string).collect()
}

mod bedrock_custom_headers_middleware {
	use super::*;

	#[test]
	fn vc1_registers_a_build_step_middleware_that_injects_the_caller_header_happy_path() {
		run(|host| async move {
			setup(&host);

			let request = drive_bedrock(&host, options(Some(headers(&[("x-custom", "v")])))).await;

			assert_eq!(request.header("x-custom"), Some("v"));
			assert!(signed_headers(&request).contains(&"x-custom".to_string()));
			assert_eq!(sorted_names(&request), expected_names(&["x-custom"]));
		});
	}

	#[test]
	fn vc2_skips_reserved_headers_case_insensitively_while_applying_allowed_ones() {
		run(|host| async move {
			setup(&host);

			let request = drive_bedrock(
				&host,
				options(Some(headers(&[
					("authorization", "evil"),
					("x-amz-date", "evil"),
					("x-allowed", "ok"),
					("Authorization", "evil2"),
					("X-Amz-Date", "evil2"),
					("HOST", "evil3"),
				]))),
			)
			.await;

			assert!(
				request.headers["authorization"].starts_with("AWS4-HMAC-SHA256 Credential=AKIAEXAMPLE/"),
				"{}",
				request.headers["authorization"]
			);
			assert_ne!(request.headers["x-amz-date"], "evil");
			assert_ne!(request.headers["x-amz-date"], "evil2");
			assert_eq!(request.header("x-allowed"), Some("ok"));
			// Mixed-case reserved keys must be skipped too: a case-sensitive guard would
			// add them back as distinct capitalised keys. Assert no such leak occurred and
			// that the only new key beyond the client's own ones is `x-allowed`.
			assert!(!request.headers.contains_key("Authorization"));
			assert!(!request.headers.contains_key("X-Amz-Date"));
			assert!(!request.headers.contains_key("HOST"));
			assert_eq!(sorted_names(&request), expected_names(&["x-allowed"]));
			assert!(signed_headers(&request).contains(&"host".to_string()));
		});
	}

	#[test]
	fn vc3_registers_no_middleware_when_headers_is_undefined() {
		run(|host| async move {
			setup(&host);

			let request = drive_bedrock(&host, options(None)).await;

			assert_eq!(sorted_names(&request), expected_names(&[]));
		});
	}

	#[test]
	fn vc3_registers_no_middleware_when_headers_is_empty() {
		run(|host| async move {
			setup(&host);

			let request = drive_bedrock(&host, options(Some(ProviderHeaders::new()))).await;

			assert_eq!(sorted_names(&request), expected_names(&[]));
		});
	}

	#[test]
	fn vc4_stream_simple_bedrock_forwards_headers_end_to_end_regression_guard() {
		run(|host| async move {
			setup(&host);
			let mut options = SimpleStreamOptions::default();
			options.base.base.headers = Some(headers(&[("x-custom", "v")]));

			let _ = stream_simple(&get_model_fixture(), &context(), Some(options))
				.result()
				.await;

			let request = only_request(&host);
			assert!(signed_headers(&request).contains(&"x-custom".to_string()));
			assert_eq!(request.header("x-custom"), Some("v"));
		});
	}
}

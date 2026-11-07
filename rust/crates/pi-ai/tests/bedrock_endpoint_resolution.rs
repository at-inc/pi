//! Port of `packages/ai/test/bedrock-endpoint-resolution.test.ts`.
//!
//! The TS `vi.mock`s `@aws-sdk/client-bedrock-runtime` and reads the `BedrockRuntimeClient` constructor config. The
//! Rust tests answer at the HTTP level and read the config's effect from the request:
//! - `config.region` is the region of the SigV4 `Credential=` scope and, without a pinned endpoint, of the URL host
//!   (`bedrock-runtime.<region>.amazonaws.com`).
//! - `config.endpoint` is the URL origin, and whether it is set at all is the `N` (custom endpoint) feature in the
//!   `user-agent` metrics section (`m/N,E,...` versus `m/E,...`). Pinning the endpoint of the region the client would
//!   pick anyway leaves the URL the same, so the feature is what tells "endpoint undefined" apart.
//! - `config.profile` is which `~/.aws/credentials` keys sign the request; the profiles get static keys there.
//! - `config.region` undefined (ambient profile) makes the real client read the profile's `region` from `~/.aws/config`
//!   (the IMDS fallback is not ported), so the ambient profile gets a region there and the request goes to it.
//! - `config.token` and `authSchemePreference` are the `Authorization: Bearer <token>` header versus a SigV4
//!   `authorization` header.
//! - The TS process env has no AWS keys under the mock; tests that need SigV4 credentials to sign set ambient
//!   `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, which change none of the asserted config fields.
//! - The constructor call count (`toHaveLength(1)`) becomes one recorded request per stream.

mod bedrock_support;

use bedrock_support::{serve, stream_response, text_stream};
use indexmap::IndexMap;
use pi_ai::compat::{get_model, stream};
use pi_ai::types::{CacheRetention, Context, Model, ProviderRequestOptions, ProviderStreamOptions, StreamOptions};
use pi_host::testing::{RecordedRequest, TestHost, run};
use serde_json::{Value, json};

const HOME: &str = "/home/pi-test";

fn context() -> Context {
	serde_json::from_value(json!({ "messages": [{ "role": "user", "content": "hello", "timestamp": 1 }] })).unwrap()
}

/// `{ cacheRetention: "none", ...options }` with the `BedrockOptions` fields in `extra` and the scoped `env`.
fn options(extra: Value, env: &[(&str, &str)]) -> ProviderStreamOptions {
	let Value::Object(extra) = extra else {
		panic!("extra must be an object");
	};
	ProviderStreamOptions {
		base: StreamOptions {
			base: ProviderRequestOptions {
				env: (!env.is_empty()).then(|| {
					env.iter()
						.map(|(key, value)| (key.to_string(), value.to_string()))
						.collect::<IndexMap<_, _>>()
				}),
				..ProviderRequestOptions::default()
			},
			cache_retention: Some(CacheRetention::None),
			..StreamOptions::default()
		},
		extra,
	}
}

/// `captureClientConfig(model, options)`: the one request the client sent.
async fn capture_request(host: &TestHost, model: &Model, options: ProviderStreamOptions) -> RecordedRequest {
	let before = host.requests().len();
	let message = stream(model, &context(), Some(options))
		.expect("a stream")
		.result()
		.await
		.expect("a final message");
	let requests = host.requests();
	assert_eq!(requests.len(), before + 1, "{:?}", message.error_message);
	requests.last().cloned().unwrap()
}

/// The `scheme://host` of the request URL.
fn origin(request: &RecordedRequest) -> String {
	url::Url::parse(&request.url).unwrap().origin().ascii_serialization()
}

/// The access key and region of the SigV4 `Credential=` scope.
fn signing_scope(request: &RecordedRequest) -> (String, String) {
	let authorization = request.header("authorization").unwrap_or_default();
	let scope = authorization
		.strip_prefix("AWS4-HMAC-SHA256 Credential=")
		.unwrap_or_else(|| panic!("expected a SigV4 authorization header, got {authorization:?}"));
	let parts: Vec<&str> = scope.split('/').collect();
	(parts[0].to_string(), parts[2].to_string())
}

/// Whether the client was given `config.endpoint` (the `N` user agent feature).
fn has_custom_endpoint(request: &RecordedRequest) -> bool {
	request.header("user-agent").unwrap_or_default().contains(" m/N,")
}

/// A home directory for `~/.aws` and a model response.
fn setup(host: &TestHost) {
	host.set_env("HOME", HOME);
	host.set_home_dir(HOME);
	serve(host, vec![stream_response(&text_stream("ok"), &[])]);
}

/// Ambient access keys, so SigV4 has credentials.
fn set_ambient_keys(host: &TestHost) {
	host.set_env("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
	host.set_env("AWS_SECRET_ACCESS_KEY", "secretexample");
}

mod bedrock_endpoint_resolution {
	use super::*;

	#[test]
	fn assigns_eu_central_1_runtime_urls_to_built_in_eu_inference_profiles() {
		let model = get_model("amazon-bedrock", "eu.anthropic.claude-sonnet-4-5-20250929-v1:0").unwrap();

		assert_eq!(model.base_url, "https://bedrock-runtime.eu-central-1.amazonaws.com");
	}

	#[test]
	fn does_not_pin_standard_aws_endpoints_when_aws_region_is_configured() {
		run(|host| async move {
			setup(&host);
			set_ambient_keys(&host);
			host.set_env("AWS_REGION", "us-east-2");
			let model = get_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(signing_scope(&request).1, "us-east-2");
			assert_eq!(origin(&request), "https://bedrock-runtime.us-east-2.amazonaws.com");
			assert!(!has_custom_endpoint(&request));
		});
	}

	#[test]
	fn derives_region_from_a_built_in_eu_endpoint_when_no_region_or_profile_is_configured() {
		run(|host| async move {
			setup(&host);
			set_ambient_keys(&host);
			let model = get_model("amazon-bedrock", "eu.anthropic.claude-sonnet-4-5-20250929-v1:0").unwrap();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(origin(&request), "https://bedrock-runtime.eu-central-1.amazonaws.com");
			assert!(has_custom_endpoint(&request));
			assert_eq!(signing_scope(&request).1, "eu-central-1");
		});
	}

	#[test]
	fn handles_missing_regions_for_explicit_scoped_and_ambient_profiles() {
		run(|host| async move {
			setup(&host);
			host.write_file(
				&format!("{HOME}/.aws/credentials"),
				"[bedrock-profile]\naws_access_key_id = AKIAEXPLICIT\naws_secret_access_key = s1\n\n[scoped-bedrock-profile]\naws_access_key_id = AKIASCOPED\naws_secret_access_key = s2\n\n[ambient-bedrock-profile]\naws_access_key_id = AKIAAMBIENT\naws_secret_access_key = s3\n",
			);
			host.write_file(
				&format!("{HOME}/.aws/config"),
				"[profile ambient-bedrock-profile]\nregion = ap-northeast-1\n",
			);
			let model = get_model("amazon-bedrock", "eu.anthropic.claude-sonnet-4-5-20250929-v1:0").unwrap();

			let request = capture_request(&host, &model, options(json!({ "profile": "bedrock-profile" }), &[])).await;

			assert_eq!(signing_scope(&request).0, "AKIAEXPLICIT");
			assert_eq!(origin(&request), "https://bedrock-runtime.eu-central-1.amazonaws.com");
			assert!(has_custom_endpoint(&request));
			assert_eq!(signing_scope(&request).1, "eu-central-1");

			let request = capture_request(
				&host,
				&model,
				options(json!({}), &[("AWS_PROFILE", "scoped-bedrock-profile")]),
			)
			.await;

			assert_eq!(signing_scope(&request).0, "AKIASCOPED");
			assert_eq!(origin(&request), "https://bedrock-runtime.eu-central-1.amazonaws.com");
			assert!(has_custom_endpoint(&request));
			assert_eq!(signing_scope(&request).1, "eu-central-1");

			host.set_env("AWS_PROFILE", "ambient-bedrock-profile");
			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(signing_scope(&request).0, "AKIAAMBIENT");
			assert!(!has_custom_endpoint(&request));
			assert_eq!(origin(&request), "https://bedrock-runtime.ap-northeast-1.amazonaws.com");
			assert_eq!(signing_scope(&request).1, "ap-northeast-1");
		});
	}

	#[test]
	fn still_passes_custom_bedrock_endpoints_through_to_the_sdk_client() {
		run(|host| async move {
			setup(&host);
			set_ambient_keys(&host);
			host.set_env("AWS_REGION", "us-west-2");
			let mut model = get_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();
			model.base_url = "https://bedrock-vpc.example.com".into();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(origin(&request), "https://bedrock-vpc.example.com");
			assert!(has_custom_endpoint(&request));
			assert_eq!(signing_scope(&request).1, "us-west-2");
		});
	}

	#[test]
	fn extracts_region_from_inference_profile_arn_regardless_of_aws_region() {
		run(|host| async move {
			setup(&host);
			set_ambient_keys(&host);
			host.set_env("AWS_REGION", "us-east-1");
			let mut model = get_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();
			model.id = "arn:aws:bedrock:us-west-2:123456789012:application-inference-profile/abc123".into();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(signing_scope(&request).1, "us-west-2");
			assert_eq!(origin(&request), "https://bedrock-runtime.us-west-2.amazonaws.com");
		});
	}

	#[test]
	fn extracts_region_from_govcloud_inference_profile_arn() {
		run(|host| async move {
			setup(&host);
			set_ambient_keys(&host);
			host.set_env("AWS_REGION", "us-east-1");
			let mut model = get_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();
			model.id = "arn:aws-us-gov:bedrock:us-gov-west-1:123456789012:application-inference-profile/abc123".into();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(signing_scope(&request).1, "us-gov-west-1");
			assert_eq!(origin(&request), "https://bedrock-runtime.us-gov-west-1.amazonaws.com");
		});
	}

	#[test]
	fn preserves_ambient_aws_auth_for_custom_model_ids_through_compat_dispatch() {
		run(|host| async move {
			setup(&host);
			host.set_env("AWS_PROFILE", "bedrock-profile");
			host.write_file(
				&format!("{HOME}/.aws/credentials"),
				"[bedrock-profile]\naws_access_key_id = AKIAPROFILE\naws_secret_access_key = s1\n",
			);
			let mut model = get_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();
			model.id = "arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/example".into();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(signing_scope(&request).0, "AKIAPROFILE");
			assert!(
				!request
					.header("authorization")
					.unwrap_or_default()
					.starts_with("Bearer "),
				"{:?}",
				request.header("authorization")
			);
		});
	}

	#[test]
	fn uses_the_generic_api_key_option_as_a_bedrock_bearer_token() {
		run(|host| async move {
			setup(&host);
			let model = get_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();
			let mut options = options(json!({}), &[]);
			options.base.base.api_key = Some("bedrock-api-key".into());

			let request = capture_request(&host, &model, options).await;

			assert_eq!(request.header("authorization"), Some("Bearer bedrock-api-key"));
		});
	}
}

//! Port of `packages/ai/test/bedrock-credentials.test.ts`.
//!
//! The TS `vi.mock`s `@aws-sdk/client-bedrock-runtime` and reads the `BedrockRuntimeClient` constructor config. The
//! Rust tests answer at the HTTP level and read the config's effect from the signed request:
//! - `config.credentials` is the access key in the SigV4 `Credential=` scope, and its missing `sessionToken` is the
//!   missing `x-amz-security-token` header. When `credentials` is undefined the client's default chain loads the
//!   profile's static keys from `~/.aws/credentials` (a configured profile skips the environment keys), so the profile's
//!   keys in the scope show both `credentials` undefined and which `profile` the client got.
//! - `config.profile` has no effect on signing while `credentials` is set, so "only an ambient profile" gives that profile
//!   a `region` in `~/.aws/config`: the request going to that region shows the client read the ambient profile.
//! - `config.profile` undefined ("no profile is configured") cannot be told apart from the `default` profile at the HTTP
//!   level; that assertion is left out.
//! - The constructor call count (`toHaveLength(1)`) becomes one recorded request per stream.

mod bedrock_support;

use bedrock_support::{serve, stream_response, text_stream};
use indexmap::IndexMap;
use pi_ai::compat::stream;
use pi_ai::providers::all::get_builtin_model;
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
	stream(model, &context(), Some(options))
		.expect("a stream")
		.result()
		.await
		.expect("a final message");
	let requests = host.requests();
	assert_eq!(requests.len(), before + 1);
	requests.last().cloned().unwrap()
}

/// The access key of the SigV4 `Credential=` scope.
fn access_key(request: &RecordedRequest) -> String {
	let authorization = request.header("authorization").unwrap_or_default();
	let scope = authorization
		.strip_prefix("AWS4-HMAC-SHA256 Credential=")
		.unwrap_or_else(|| panic!("expected a SigV4 authorization header, got {authorization:?}"));
	scope.split('/').next().unwrap().to_string()
}

/// The host of the request URL.
fn url_host(request: &RecordedRequest) -> String {
	url::Url::parse(&request.url).unwrap().host_str().unwrap().to_string()
}

/// `vi.stubEnv` of the ambient access keys, a home directory for `~/.aws`, and a model response.
fn setup(host: &TestHost) {
	host.set_env("AWS_ACCESS_KEY_ID", "AKIAEXAMPLE");
	host.set_env("AWS_SECRET_ACCESS_KEY", "secretexample");
	host.set_env("HOME", HOME);
	host.set_home_dir(HOME);
	serve(host, vec![stream_response(&text_stream("ok"), &[])]);
}

mod bedrock_credential_priority {
	use super::*;

	#[test]
	fn prefers_explicit_and_scoped_profiles_over_ambient_aws_access_keys() {
		run(|host| async move {
			setup(&host);
			host.write_file(
				&format!("{HOME}/.aws/credentials"),
				"[explicit-profile]\naws_access_key_id = AKIAEXPLICIT\naws_secret_access_key = explicitsecret\n\n[scoped-profile]\naws_access_key_id = AKIASCOPED\naws_secret_access_key = scopedsecret\n",
			);
			let model = get_builtin_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();

			let request = capture_request(&host, &model, options(json!({ "profile": "explicit-profile" }), &[])).await;

			assert_eq!(access_key(&request), "AKIAEXPLICIT");

			let request =
				capture_request(&host, &model, options(json!({}), &[("AWS_PROFILE", "scoped-profile")])).await;

			assert_eq!(access_key(&request), "AKIASCOPED");
		});
	}

	#[test]
	fn uses_ambient_aws_access_keys_when_no_profile_is_configured() {
		run(|host| async move {
			setup(&host);
			let model = get_builtin_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(access_key(&request), "AKIAEXAMPLE");
			assert_eq!(request.header("x-amz-security-token"), None);
		});
	}

	#[test]
	fn uses_ambient_aws_access_keys_when_only_an_ambient_profile_is_set() {
		run(|host| async move {
			setup(&host);
			host.set_env("AWS_PROFILE", "ambient-profile");
			host.write_file(
				&format!("{HOME}/.aws/config"),
				"[profile ambient-profile]\nregion = eu-west-3\n",
			);
			host.write_file(
				&format!("{HOME}/.aws/credentials"),
				"[ambient-profile]\naws_access_key_id = AKIAAMBIENTPROFILE\naws_secret_access_key = ambientsecret\n",
			);
			let model = get_builtin_model("amazon-bedrock", "us.anthropic.claude-opus-4-8").unwrap();

			let request = capture_request(&host, &model, options(json!({}), &[])).await;

			assert_eq!(url_host(&request), "bedrock-runtime.eu-west-3.amazonaws.com");
			assert_eq!(access_key(&request), "AKIAEXAMPLE");
			assert_eq!(request.header("x-amz-security-token"), None);
		});
	}
}

//! Port of `packages/ai/test/node-http-proxy.test.ts`.
//!
//! `process.env` is the `TestHost` environment, which starts empty in every test, so `resetProxyEnv` and the
//! `afterEach` restore have nothing to do.

use pi_ai::types::ProviderEnv;
use pi_ai::utils::node_http_proxy::{UNSUPPORTED_PROXY_PROTOCOL_MESSAGE, resolve_http_proxy_url_for_target};
use pi_host::testing::run;

fn resolve(target_url: &str) -> Option<String> {
	resolve_http_proxy_url_for_target(target_url, None)
		.expect("resolves")
		.map(|url| url.to_string())
}

mod node_http_proxy_resolution {
	use super::*;

	#[test]
	fn respects_no_proxy_exclusions() {
		run(|host| async move {
			host.set_env("HTTPS_PROXY", "http://proxy.example:8080");
			host.set_env("NO_PROXY", "bedrock-runtime.us-east-1.amazonaws.com");

			assert_eq!(resolve("https://bedrock-runtime.us-east-1.amazonaws.com"), None);
		});
	}

	#[test]
	fn resolves_http_and_https_proxy_urls() {
		run(|host| async move {
			host.set_env("HTTPS_PROXY", "http://proxy.example:8080");

			assert_eq!(
				resolve("https://bedrock-runtime.us-east-1.amazonaws.com").as_deref(),
				Some("http://proxy.example:8080/")
			);
		});
	}

	#[test]
	fn prefers_scoped_proxy_env_aliases_before_process_env_aliases() {
		run(|host| async move {
			host.set_env("https_proxy", "http://process-proxy.example:8080");

			let env = ProviderEnv::from([(
				"HTTPS_PROXY".to_string(),
				"http://scoped-proxy.example:8080".to_string(),
			)]);
			assert_eq!(
				resolve_http_proxy_url_for_target("https://bedrock-runtime.us-east-1.amazonaws.com", Some(&env))
					.expect("resolves")
					.map(|url| url.to_string())
					.as_deref(),
				Some("http://scoped-proxy.example:8080/")
			);
		});
	}

	#[test]
	fn rejects_socks_and_pac_proxy_urls_explicitly() {
		run(|host| async move {
			host.set_env("HTTPS_PROXY", "socks5://proxy.example:1080");

			let error = resolve_http_proxy_url_for_target("https://bedrock-runtime.us-east-1.amazonaws.com", None)
				.expect_err("rejects");
			assert!(
				error.message.contains(UNSUPPORTED_PROXY_PROTOCOL_MESSAGE),
				"{}",
				error.message
			);
		});
	}

	#[test]
	fn handles_subdomain_wildcards_ipv6_and_ports_in_no_proxy() {
		run(|host| async move {
			host.set_env("HTTPS_PROXY", "http://proxy.example:8080");
			host.set_env(
				"NO_PROXY",
				"example.com, .wildcard.org, *.star.net, ::1, [2001:db8::1], 127.0.0.1:8080",
			);

			assert_eq!(resolve("https://example.com"), None);
			assert_eq!(resolve("https://api.example.com"), None);
			assert_eq!(resolve("https://wildcard.org"), None);
			assert_eq!(resolve("https://api.wildcard.org"), None);
			assert_eq!(resolve("https://star.net"), None);
			assert_eq!(resolve("https://api.star.net"), None);
			assert_eq!(
				resolve("https://notexample.com").as_deref(),
				Some("http://proxy.example:8080/")
			);

			assert_eq!(resolve("https://[::1]:80"), None);
			assert_eq!(resolve("https://[2001:db8::1]"), None);
			assert_eq!(resolve("https://127.0.0.1:8080"), None);
			assert_eq!(
				resolve("https://127.0.0.1:3000").as_deref(),
				Some("http://proxy.example:8080/")
			);
		});
	}
}

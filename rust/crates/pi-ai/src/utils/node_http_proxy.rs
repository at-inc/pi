//! Port of `packages/ai/src/utils/node-http-proxy.ts`.
//! PORT-STATUS: reviewed
//!
//! `targetUrl: string | URL` is a `&str`; the proxy URL is a `url::Url`, which implements the WHATWG URL standard
//! like Node's `URL` (`Url::as_str` is `URL.href`). An invalid proxy URL reports Node's `TypeError` message,
//! `Invalid URL`.

use url::Url;

use pi_host::{PiError, PiResult};

use super::provider_env::get_provider_env_value;
use super::text::js_trim;
use crate::types::ProviderEnv;

fn default_proxy_port(protocol: &str) -> Option<f64> {
	match protocol {
		"ftp" => Some(21.0),
		"gopher" => Some(70.0),
		"http" => Some(80.0),
		"https" => Some(443.0),
		"ws" => Some(80.0),
		"wss" => Some(443.0),
		_ => None,
	}
}

fn get_proxy_env(key: &str, env: Option<&ProviderEnv>) -> String {
	let lowercase_key = key.to_lowercase();
	let uppercase_key = key.to_uppercase();
	let scoped = |key: &str| {
		env.and_then(|env| env.get(key))
			.filter(|value| !value.is_empty())
			.cloned()
	};
	scoped(&lowercase_key)
		.or_else(|| scoped(&uppercase_key))
		.or_else(|| get_provider_env_value(&lowercase_key, None))
		.or_else(|| get_provider_env_value(&uppercase_key, None))
		.unwrap_or_default()
}

fn parse_proxy_target_url(target_url: &str) -> Option<Url> {
	Url::parse(target_url).ok()
}

fn strip_brackets(host: &str) -> &str {
	if host.starts_with('[') && host.ends_with(']') && host.len() >= 2 {
		&host[1..host.len() - 1]
	} else {
		host
	}
}

/// `Number.parseInt(text, 10)`.
fn parse_int(text: &str) -> f64 {
	let text = text.trim_start_matches(|ch: char| (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}');
	let (negative, digits) = match text.as_bytes().first() {
		Some(b'-') => (true, &text[1..]),
		Some(b'+') => (false, &text[1..]),
		_ => (false, text),
	};
	let digits = &digits[..digits.bytes().take_while(u8::is_ascii_digit).count()];
	if digits.is_empty() {
		return f64::NAN;
	}
	let value: f64 = digits.parse().unwrap_or(f64::NAN);
	if negative { -value } else { value }
}

struct NoProxyEntry {
	host: String,
	port: f64,
}

fn parse_no_proxy_entry(entry: &str) -> Option<NoProxyEntry> {
	let trimmed = js_trim(entry).to_lowercase();
	if trimmed.is_empty() {
		return None;
	}

	if trimmed.starts_with('[')
		&& let Some(closing_bracket) = trimmed.find(']')
	{
		let host = trimmed[1..closing_bracket].to_string();
		let rest = &trimmed[closing_bracket + 1..];
		if let Some(port) = rest.strip_prefix(':') {
			let port = parse_int(port);
			return Some(NoProxyEntry {
				host,
				port: if port.is_nan() { 0.0 } else { port },
			});
		}
		return Some(NoProxyEntry { host, port: 0.0 });
	}

	if trimmed.matches(':').count() >= 2 {
		return Some(NoProxyEntry {
			host: trimmed,
			port: 0.0,
		});
	}

	if let Some(colon_index) = trimmed.rfind(':')
		&& Some(colon_index) == trimmed.find(':')
	{
		let port = parse_int(&trimmed[colon_index + 1..]);
		if !port.is_nan() {
			return Some(NoProxyEntry {
				host: trimmed[..colon_index].to_string(),
				port,
			});
		}
	}

	Some(NoProxyEntry {
		host: trimmed,
		port: 0.0,
	})
}

fn should_proxy_hostname(hostname: &str, port: f64, env: Option<&ProviderEnv>) -> bool {
	let no_proxy = get_proxy_env("no_proxy", env).to_lowercase();
	if no_proxy.is_empty() {
		return true;
	}
	if no_proxy == "*" {
		return false;
	}

	let lowercase_hostname = hostname.to_lowercase();
	let normalized_target_host = strip_brackets(&lowercase_hostname);

	no_proxy
		.split(|ch: char| ch == ',' || (ch.is_whitespace() && ch != '\u{85}') || ch == '\u{feff}')
		.all(|entry| {
			let Some(parsed) = parse_no_proxy_entry(entry) else {
				return true;
			};

			if parsed.port != 0.0 && !parsed.port.is_nan() && parsed.port != port {
				return true;
			}

			let mut domain = strip_brackets(&parsed.host);
			if let Some(rest) = domain.strip_prefix("*.") {
				domain = rest;
			} else if let Some(rest) = domain.strip_prefix('.').or_else(|| domain.strip_prefix('*')) {
				domain = rest;
			}

			if domain.is_empty() {
				return true;
			}

			if normalized_target_host == domain {
				return false;
			}

			if normalized_target_host.ends_with(&format!(".{domain}")) {
				return false;
			}

			true
		})
}

fn get_proxy_for_url(target_url: &str, env: Option<&ProviderEnv>) -> String {
	let Some(parsed_url) = parse_proxy_target_url(target_url) else {
		return String::new();
	};
	let Some(host) = parsed_url.host_str().filter(|host| !host.is_empty()) else {
		return String::new();
	};

	let protocol = parsed_url.scheme();
	let hostname = strip_brackets(host);
	let port = parsed_url
		.port()
		.map(f64::from)
		.filter(|port| *port != 0.0)
		.or_else(|| default_proxy_port(protocol))
		.unwrap_or(0.0);
	if !should_proxy_hostname(hostname, port, env) {
		return String::new();
	}

	let mut proxy = get_proxy_env(&format!("{protocol}_proxy"), env);
	if proxy.is_empty() {
		proxy = get_proxy_env("all_proxy", env);
	}
	if !proxy.is_empty() && !proxy.contains("://") {
		proxy = format!("{protocol}://{proxy}");
	}
	proxy
}

pub const UNSUPPORTED_PROXY_PROTOCOL_MESSAGE: &str =
	"Unsupported proxy protocol. SOCKS and PAC proxy URLs are not supported; use an HTTP or HTTPS proxy URL.";

pub fn resolve_http_proxy_url_for_target(target_url: &str, env: Option<&ProviderEnv>) -> PiResult<Option<Url>> {
	let proxy = get_proxy_for_url(target_url, env);
	if proxy.is_empty() {
		return Ok(None);
	}

	let Ok(proxy_url) = Url::parse(&proxy) else {
		return Err(PiError::new(format!(
			"Invalid proxy URL {}: Invalid URL",
			pi_host::js::json_stringify(&proxy)
		)));
	};

	if proxy_url.scheme() != "http" && proxy_url.scheme() != "https" {
		return Err(PiError::new(format!(
			"{UNSUPPORTED_PROXY_PROTOCOL_MESSAGE} Got {}:",
			proxy_url.scheme()
		)));
	}

	Ok(Some(proxy_url))
}

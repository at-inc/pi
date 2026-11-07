//! Port of `packages/ai/src/auth/oauth/trusted-url.ts`.
//! PORT-STATUS: ported
//!
//! The WHATWG `URL` parser is the `url` crate, which implements the same standard. It is stricter on invalid
//! punycode labels: `https://xn--a.com/` is rejected, where Node keeps it.

use serde_json::Value;

/// The `http:` or `https:` URL `value` names, normalized, or null. Login flows open server-supplied URLs with the
/// platform URL handler, which would also launch a `file:` URL or a custom protocol handler. Normalizing
/// percent-encodes control characters, so the URL can also be embedded in terminal hyperlinks.
pub fn trusted_http_url(value: &Value) -> Option<String> {
	let Value::String(value) = value else {
		return None;
	};
	let url = ::url::Url::parse(value).ok()?;
	if url.scheme() == "https" || url.scheme() == "http" {
		Some(url.as_str().to_string())
	} else {
		None
	}
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn keeps_only_http_urls_and_normalizes_them() {
		assert_eq!(
			trusted_http_url(&json!("HTTPS://Example.com/a b")).as_deref(),
			Some("https://example.com/a%20b")
		);
		assert_eq!(
			trusted_http_url(&json!("http://x.test")).as_deref(),
			Some("http://x.test/")
		);
		assert_eq!(trusted_http_url(&json!("file:///etc/passwd")), None);
		assert_eq!(trusted_http_url(&json!("not a url")), None);
		assert_eq!(trusted_http_url(&json!(1)), None);
	}
}

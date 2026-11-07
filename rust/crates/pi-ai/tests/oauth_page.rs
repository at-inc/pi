//! Port of `packages/ai/test/oauth-page.test.ts`.
//!
//! `it.each` over the hostile inputs is a loop inside one test per `it.each`.

use pi_ai::utils::oauth_page::{oauth_error_html, oauth_success_html};

struct HostileInput {
	name: &'static str,
	input: &'static str,
	escaped: &'static str,
}

const HOSTILE_INPUTS: [HostileInput; 5] = [
	HostileInput {
		name: "repeated HTML special characters",
		input: r#"&<>"' &<>"'"#,
		escaped: "&amp;&lt;&gt;&quot;&#39; &amp;&lt;&gt;&quot;&#39;",
	},
	HostileInput {
		name: "script tags that close the message element",
		input: r#"</p><script>alert("oauth")</script><p>"#,
		escaped: "&lt;/p&gt;&lt;script&gt;alert(&quot;oauth&quot;)&lt;/script&gt;&lt;p&gt;",
	},
	HostileInput {
		name: "an image with an event handler",
		input: r#"<img src=x onerror='alert("oauth")'>"#,
		escaped: "&lt;img src=x onerror=&#39;alert(&quot;oauth&quot;)&#39;&gt;",
	},
	HostileInput {
		name: "SVG tags that close the details element",
		input: r#"</div><svg onload="alert(1)"></svg><div>"#,
		escaped: "&lt;/div&gt;&lt;svg onload=&quot;alert(1)&quot;&gt;&lt;/svg&gt;&lt;div&gt;",
	},
	HostileInput {
		name: "literal HTML entities",
		input: "&lt;script&gt; &amp; &#39; &#x3c;script&#x3e;",
		escaped: "&amp;lt;script&amp;gt; &amp;amp; &amp;#39; &amp;#x3c;script&amp;#x3e;",
	},
];

mod oauth_page_html_escaping {
	use super::*;

	#[test]
	fn escapes_name_in_success_messages() {
		for HostileInput { name, input, escaped } in HOSTILE_INPUTS {
			let expected = oauth_success_html("message-placeholder").replacen(
				"<p>message-placeholder</p>",
				&format!("<p>{escaped}</p>"),
				1,
			);
			assert_eq!(oauth_success_html(input), expected, "{name}");
		}
	}

	#[test]
	fn escapes_name_in_error_messages() {
		for HostileInput { name, input, escaped } in HOSTILE_INPUTS {
			let expected = oauth_error_html("message-placeholder", Some("Details")).replacen(
				"<p>message-placeholder</p>",
				&format!("<p>{escaped}</p>"),
				1,
			);
			assert_eq!(oauth_error_html(input, Some("Details")), expected, "{name}");
		}
	}

	#[test]
	fn escapes_name_in_error_details() {
		for HostileInput { name, input, escaped } in HOSTILE_INPUTS {
			let expected = oauth_error_html("Sign-in failed.", Some("details-placeholder")).replacen(
				r#"<div class="details">details-placeholder</div>"#,
				&format!(r#"<div class="details">{escaped}</div>"#),
				1,
			);
			assert_eq!(oauth_error_html("Sign-in failed.", Some(input)), expected, "{name}");
		}
	}

	#[test]
	fn preserves_ordinary_text_unicode_whitespace_and_page_markup() {
		let text = "Signed in to Café 한글.\n\tRetry later. $& $` $'";
		let html = oauth_error_html("Sign-in failed.", Some(text));
		assert!(html.contains("<div class=\"details\">Signed in to Café 한글.\n\tRetry later. $&amp; $` $&#39;</div>"));
		assert!(html.contains("<!doctype html>"));
		assert!(html.contains("<title>Authentication failed</title>"));
		assert!(html.contains("<h1>Authentication failed</h1>"));
		assert!(html.contains(r#"<svg xmlns="http://www.w3.org/2000/svg""#));
		assert!(html.contains(r##"fill="#F09082""##));
		assert!(html.contains(r##"fill="#4D9ABF""##));
		assert!(html.contains(r##"fill="#F1BE58""##));
	}

	#[test]
	fn omits_absent_or_empty_details() {
		assert!(!oauth_error_html("Sign-in failed.", None).contains(r#"<div class="details">"#));
		assert_eq!(
			oauth_error_html("Sign-in failed.", Some("")),
			oauth_error_html("Sign-in failed.", None)
		);
		assert!(!oauth_success_html("Signed in.").contains(r#"<div class="details">"#));
	}
}

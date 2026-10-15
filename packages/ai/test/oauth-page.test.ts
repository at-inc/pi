import { describe, expect, it } from "vitest";
import { oauthErrorHtml, oauthSuccessHtml } from "../src/utils/oauth-page.ts";

const hostileInputs = [
	{
		name: "repeated HTML special characters",
		input: `&<>"' &<>"'`,
		escaped: "&amp;&lt;&gt;&quot;&#39; &amp;&lt;&gt;&quot;&#39;",
	},
	{
		name: "script tags that close the message element",
		input: '</p><script>alert("oauth")</script><p>',
		escaped: "&lt;/p&gt;&lt;script&gt;alert(&quot;oauth&quot;)&lt;/script&gt;&lt;p&gt;",
	},
	{
		name: "an image with an event handler",
		input: `<img src=x onerror='alert("oauth")'>`,
		escaped: "&lt;img src=x onerror=&#39;alert(&quot;oauth&quot;)&#39;&gt;",
	},
	{
		name: "SVG tags that close the details element",
		input: '</div><svg onload="alert(1)"></svg><div>',
		escaped: "&lt;/div&gt;&lt;svg onload=&quot;alert(1)&quot;&gt;&lt;/svg&gt;&lt;div&gt;",
	},
	{
		name: "literal HTML entities",
		input: "&lt;script&gt; &amp; &#39; &#x3c;script&#x3e;",
		escaped: "&amp;lt;script&amp;gt; &amp;amp; &amp;#39; &amp;#x3c;script&amp;#x3e;",
	},
];

describe("OAuth page HTML escaping", () => {
	it.each(hostileInputs)("escapes $name in success messages", ({ input, escaped }) => {
		const expected = oauthSuccessHtml("message-placeholder").replace(
			"<p>message-placeholder</p>",
			() => `<p>${escaped}</p>`,
		);
		expect(oauthSuccessHtml(input)).toBe(expected);
	});

	it.each(hostileInputs)("escapes $name in error messages", ({ input, escaped }) => {
		const expected = oauthErrorHtml("message-placeholder", "Details").replace(
			"<p>message-placeholder</p>",
			() => `<p>${escaped}</p>`,
		);
		expect(oauthErrorHtml(input, "Details")).toBe(expected);
	});

	it.each(hostileInputs)("escapes $name in error details", ({ input, escaped }) => {
		const expected = oauthErrorHtml("Sign-in failed.", "details-placeholder").replace(
			'<div class="details">details-placeholder</div>',
			() => `<div class="details">${escaped}</div>`,
		);
		expect(oauthErrorHtml("Sign-in failed.", input)).toBe(expected);
	});

	it("preserves ordinary text, Unicode, whitespace, and page markup", () => {
		const text = "Signed in to Café 한글.\n\tRetry later. $& $` $'";
		const html = oauthErrorHtml("Sign-in failed.", text);
		expect(html).toContain('<div class="details">Signed in to Café 한글.\n\tRetry later. $&amp; $` $&#39;</div>');
		expect(html).toContain("<!doctype html>");
		expect(html).toContain("<title>Authentication failed</title>");
		expect(html).toContain("<h1>Authentication failed</h1>");
		expect(html).toContain('<svg xmlns="http://www.w3.org/2000/svg"');
		expect(html).toContain('fill="#F09082"');
		expect(html).toContain('fill="#4D9ABF"');
		expect(html).toContain('fill="#F1BE58"');
	});

	it("omits absent or empty details", () => {
		expect(oauthErrorHtml("Sign-in failed.")).not.toContain('<div class="details">');
		expect(oauthErrorHtml("Sign-in failed.", "")).toBe(oauthErrorHtml("Sign-in failed."));
		expect(oauthSuccessHtml("Signed in.")).not.toContain('<div class="details">');
	});
});

import assert from "node:assert/strict";
import { test } from "node:test";
import { runInNewContext } from "node:vm";
import { Browser, browserTests } from "./browser.ts";
import { jsString } from "./helpers.ts";

test("JavaScript string literals preserve quotes, escapes, Unicode and repeated delimiters", () => {
	for (const value of [
		"",
		"quotes: \" ' `; slashes: \\ \\\\",
		`interpolation: \${value}`,
		"line\nreturn\rtab\tzero\0",
		"<>&<>&\u2028\u2029\u2028\u2029",
		"한글 \u{1d11e} \\u003c \\u2028",
		"\ud800",
	]) {
		const encoded = jsString(value);
		assert.doesNotMatch(encoded, /[<>&\u2028\u2029]/u);
		assert.equal(JSON.parse(encoded), value);
		assert.equal(runInNewContext(`(${encoded})`), value);
	}
	assert.equal(jsString("<>&\u2028\u2029"), '"\\u003c\\u003e\\u0026\\u2028\\u2029"');
});

test("JavaScript string literals cannot escape into the surrounding function", () => {
	const value = '"); globalThis.injected = true; // </script><script>globalThis.injected = true</script>';
	const context = { injected: false };
	const result = runInNewContext(`(() => { const value = ${jsString(value)}; return value; })()`, context);
	assert.equal(result, value);
	assert.equal(context.injected, false);
});

test("encoded strings remain data inside HTML script elements", { skip: !browserTests }, async (t) => {
	const browser = await Browser.start();
	t.after(() => browser.close());
	const value = "</script><script>window.injected = true</script><!-- <>&\u2028\u2029";
	const page = `<!doctype html><script>window.value = ${jsString(value)}; window.completed = true;</script>`;
	await browser.open(`data:text/html;charset=utf-8,${encodeURIComponent(page)}`);
	await browser.wait("window.completed === true");
	assert.equal(await browser.evaluate("window.value"), value);
	assert.equal(await browser.evaluate("window.injected === undefined"), true);
	assert.equal(await browser.evaluate("document.scripts.length"), 1);
});

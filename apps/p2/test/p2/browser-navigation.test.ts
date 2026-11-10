import assert from "node:assert/strict";
import { createServer } from "node:http";
import { type TestContext, test } from "node:test";
import { Browser, browserTests } from "./browser.ts";

async function fixture(t: TestContext) {
	const server = createServer((request, response) => {
		response.writeHead(200, { "Content-Type": "text/html" });
		if (request.url === "/redirect") {
			response.write('<!doctype html><script>location.replace("/final")</script>');
			return;
		}
		response.end("<!doctype html><title>Loaded</title><p>Ready</p>");
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	t.after(() => new Promise<void>((resolve) => server.close(() => resolve())));
	const address = server.address();
	assert(address && typeof address !== "string");
	const browser = await Browser.start();
	t.after(() => browser.close());
	return { browser, origin: `http://127.0.0.1:${address.port}` };
}

test(
	"browser open follows a redirect that cancels the first document before DOMContentLoaded",
	{ skip: !browserTests },
	async (t) => {
		const { browser, origin } = await fixture(t);
		await browser.open(`${origin}/start`);
		await browser.open(`${origin}/redirect`);
		assert.equal(await browser.evaluate("location.pathname"), "/final");
		assert.equal(await browser.evaluate("document.title"), "Loaded");
	},
);

test(
	"browser predicates survive navigation but still reject script and protocol errors",
	{ skip: !browserTests },
	async (t) => {
		const { browser, origin } = await fixture(t);
		await browser.open(`${origin}/start`);
		const call = browser.call.bind(browser);
		let attempts = 0;
		const navigating = t.mock.method(browser, "call", (method: string, params: object = {}, sessionId?: string) => {
			if (method !== "Runtime.evaluate") return call(method, params, sessionId);
			attempts++;
			return call(
				method,
				attempts === 1
					? {
							expression: 'new Promise(() => { location.replace("/final"); })',
							returnByValue: true,
							awaitPromise: true,
						}
					: params,
				sessionId,
			);
		});
		await browser.wait('location.pathname === "/final"');
		assert(attempts > 1, "the predicate was retried after navigation destroyed its pending evaluation");
		navigating.mock.restore();
		assert.equal(await browser.evaluate("location.pathname"), "/final");
		await assert.rejects(browser.wait('(() => { throw new Error("predicate failure"); })()'), /predicate failure/);
		const failed = t.mock.method(browser, "call", async () => {
			throw new Error("protocol failure");
		});
		await assert.rejects(browser.wait("true"), /protocol failure/);
		failed.mock.restore();
	},
);

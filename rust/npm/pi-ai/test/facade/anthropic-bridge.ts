// Bridges a mocked `@anthropic-ai/sdk` to the core. The TS tests mock the SDK class (`vi.mock("@anthropic-ai/sdk")`)
// and assert on the client options and params the TS passed to it; the core speaks HTTP instead. When this module sees
// a mocked SDK it replaces the host's fetch with a bridge that turns each Messages request back into the call the TS
// makes, `new Anthropic(clientOptions).beta.messages.create(params, requestOptions).asResponse()`, and returns the
// mock's response (or a thrown error with a status as an error response) to the core. Imported by the shims, so it
// sees the test file's mocks.
//
// The wire loses some of the client options, so they are rebuilt from the request:
// - `x-api-key` is `apiKey`. A bearer `authorization` is `authToken` when the TS would pass it as one (an OAuth
//   token, `sk-ant-oat...`, or a GitHub Copilot request, which carries `copilot-integration-id`), and
//   `defaultHeaders.Authorization` otherwise.
// - The other headers are `defaultHeaders`, named in Train-Case (the core sends lower case), without the headers the
//   SDK sets itself. `anthropic-beta` is `params.betas` and `anthropic-version` the request option's header.
// - A workload identity federation `config` cannot be rebuilt: the core exchanges the identity token itself, so a
//   mocked federation request fails before it reaches this bridge.

import Anthropic from "@anthropic-ai/sdk";
import { host } from "@at-inc/pi-core";

interface FakeAnthropic {
	beta: { messages: { create(params: unknown, options: unknown): { asResponse(): Promise<Response> } } };
}

type FakeAnthropicClass = new (options: unknown) => FakeAnthropic;

// The real SDK class carries its error classes as statics; a test's fake class does not.
const mocked = typeof (Anthropic as unknown as { APIError?: unknown }).APIError !== "function";

/** The path the core's Messages requests end with, before the query. */
const MESSAGES_PATH = "/v1/messages";

/** Headers the SDK sets itself, or derives from client options and params the bridge passes separately. */
const SDK_HEADERS = /^(content-type|x-stainless-.*|x-api-key|authorization|anthropic-beta|anthropic-version)$/;

if (mocked) {
	const realFetch = host.fetch;
	host.fetch = async (url, init) => {
		const path = new URL(url).pathname;
		if (!path.endsWith(MESSAGES_PATH)) return realFetch(url, init);
		const headers = new Headers(init.headers as ConstructorParameters<typeof Headers>[0]);
		const authorization = headers.get("authorization") ?? undefined;
		const token = authorization?.replace(/^Bearer /, "");
		const bearer = token !== undefined && (token.includes("sk-ant-oat") || headers.has("copilot-integration-id"));
		const defaultHeaders: Record<string, string> = {};
		for (const [name, value] of headers) {
			if (SDK_HEADERS.test(name) || (name === "user-agent" && value.startsWith("Anthropic/JS"))) continue;
			defaultHeaders[
				name.replace(/(^|-)([a-z])/g, (_, dash: string, letter: string) => dash + letter.toUpperCase())
			] = value;
		}
		if (authorization !== undefined && !bearer) defaultHeaders.Authorization = authorization;
		const client = new (Anthropic as unknown as FakeAnthropicClass)({
			apiKey: headers.get("x-api-key"),
			authToken: bearer ? token : null,
			baseURL: url.slice(0, url.indexOf(MESSAGES_PATH)),
			dangerouslyAllowBrowser: true,
			defaultHeaders,
		});
		const betas = headers
			.get("anthropic-beta")
			?.split(",")
			.map((beta) => beta.trim());
		const params = { ...JSON.parse(init.body as string), ...(betas ? { betas: [...new Set(betas)] } : {}) };
		const version = headers.get("anthropic-version");
		const requestOptions = {
			...(version !== null ? { headers: { "anthropic-version": version } } : {}),
			...(init.signal ? { signal: init.signal } : {}),
			maxRetries: 0,
		};
		try {
			return await client.beta.messages.create(params, requestOptions).asResponse();
		} catch (error) {
			const status = (error as { status?: unknown }).status;
			if (typeof status !== "number") throw error;
			const type = (error as { type?: unknown }).type ?? "api_error";
			return new Response(JSON.stringify({ type: "error", error: { type, message: (error as Error).message } }), {
				status,
				headers: (error as { headers?: ResponseInit["headers"] }).headers,
			});
		}
	};
}

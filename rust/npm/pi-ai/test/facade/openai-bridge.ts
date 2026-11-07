// Bridges a mocked `openai` SDK to the core. The TS tests mock the SDK class (`vi.mock("openai")`) and assert on what
// the TS passed to it; the core speaks HTTP instead. When this module sees a mocked SDK it replaces the host's fetch
// with a bridge that turns each Chat Completions or Responses request back into the SDK call the TS makes (client
// options from the URL and headers, params from the body, request options `{ signal, maxRetries: 0 }`) and the mock's
// chunks (or, for a non-streaming request, its response object) or thrown error back into an HTTP response; a thrown
// `APIError`'s `error` is the body's `error`, as the SDK parses it. Imported by the shims, so it sees the test file's mocks.
//
// The wire cannot tell `apiKey` from an `Authorization` the caller passed in `headers`, and the core sends header
// names in lower case, so a request's authorization becomes both `apiKey` and `defaultHeaders.Authorization`; an
// absent one becomes `defaultHeaders.Authorization: null`, which is how the TS drops it.

import { host } from "@at-inc/pi-core";
import OpenAI from "openai";

interface FakeEndpoint {
	create(params: unknown, options: unknown): PromiseLike<unknown> & { withResponse?(): Promise<unknown> };
}

interface FakeClient {
	chat: { completions: FakeEndpoint };
	responses: FakeEndpoint;
}

type FakeOpenAI = new (options: unknown) => FakeClient;

// The real SDK class carries its error classes as statics; a test's fake class does not.
const mocked = typeof (OpenAI as unknown as { APIError?: unknown }).APIError !== "function";

/** The SDK endpoints the core's OpenAI adapters call, by URL path suffix. */
const ENDPOINTS: Array<[string, (client: FakeClient) => FakeEndpoint]> = [
	["/chat/completions", (client) => client.chat.completions],
	["/responses", (client) => client.responses],
];

/** Headers the SDK adds itself, which the TS never passes as `defaultHeaders`. */
const SDK_HEADERS = /^(content-type|accept|x-stainless-.*)$/i;

if (mocked) {
	const realFetch = host.fetch;
	host.fetch = async (url, init) => {
		const endpoint = ENDPOINTS.find(([path]) => url.endsWith(path));
		if (!endpoint) return realFetch(url, init);
		const [path, select] = endpoint;
		const headers = (init.headers ?? {}) as Record<string, string>;
		const authorization = Object.entries(headers).find(([name]) => name.toLowerCase() === "authorization")?.[1];
		const defaultHeaders: Record<string, string | null> = Object.fromEntries(
			Object.entries(headers).filter(
				([name, value]) =>
					!SDK_HEADERS.test(name) &&
					name.toLowerCase() !== "authorization" &&
					!(name.toLowerCase() === "user-agent" && value.startsWith("OpenAI/JS")),
			),
		);
		defaultHeaders.Authorization = authorization ?? null;
		const client = new (OpenAI as unknown as FakeOpenAI)({
			apiKey: authorization?.replace(/^Bearer /, ""),
			baseURL: url.slice(0, -path.length),
			dangerouslyAllowBrowser: true,
			defaultHeaders,
		});
		const params = JSON.parse(init.body as string);
		try {
			const request = select(client).create(params, { signal: init.signal, maxRetries: 0 });
			const { data, response } = (await (request.withResponse
				? request.withResponse()
				: request.then((data) => ({ data })))) as {
				data: unknown;
				response?: { status?: number; headers?: ResponseInit["headers"] };
			};
			if (!params.stream) {
				const headers = new Headers(response?.headers);
				if (!headers.has("content-type")) headers.set("content-type", "application/json");
				return new Response(JSON.stringify(data), { status: response?.status ?? 200, headers });
			}
			return new Response(sse(data), {
				status: response?.status ?? 200,
				headers: response?.headers ?? { "content-type": "text/event-stream" },
			});
		} catch (error) {
			const status = (error as { status?: unknown }).status;
			if (typeof status !== "number") throw error;
			const body = (error as { error?: unknown }).error ?? { message: (error as Error).message };
			return new Response(JSON.stringify({ error: body }), {
				status,
				headers: (error as { headers?: ResponseInit["headers"] }).headers,
			});
		}
	};
}

/** The SSE body of the mock's stream: one `data:` event per chunk, then `[DONE]`. */
function sse(data: unknown): ReadableStream<Uint8Array> {
	const encoder = new TextEncoder();
	return new ReadableStream({
		async start(controller) {
			try {
				for await (const chunk of data as AsyncIterable<unknown>) {
					controller.enqueue(encoder.encode(`data: ${JSON.stringify(chunk)}\n\n`));
				}
				controller.enqueue(encoder.encode("data: [DONE]\n\n"));
				controller.close();
			} catch (error) {
				controller.error(error);
			}
		},
	});
}

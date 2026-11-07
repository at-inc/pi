// Bridges a mocked `openai` SDK to the core. The TS tests mock the SDK class (`vi.mock("openai")`) and assert on what
// the TS passed to it; the core speaks HTTP instead. When this module sees a mocked SDK it replaces the host's fetch
// with a bridge that turns each Chat Completions or Responses request back into the SDK call the TS makes (client
// options from the URL and headers, params from the body, request options `{ signal, maxRetries: 0 }`) and the mock's
// chunks (or, for a non-streaming request, its response object) or thrown error back into an HTTP response; a thrown
// `APIError`'s `error` is the body's `error`, as the SDK parses it. Imported by the shims, so it sees the test file's mocks.
// A request with an `api-version` query is an Azure request: it goes to the mock's `AzureOpenAI` class with
// `apiVersion` and the `api-key` header as `apiKey`, as the TS constructs that client.
//
// The wire cannot tell `apiKey` from an `Authorization` the caller passed in `headers`, and the core sends header
// names in lower case, so a request's authorization becomes both `apiKey` and `defaultHeaders.Authorization`; an
// absent one becomes `defaultHeaders.Authorization: null`, which is how the TS drops it.

import { host } from "@at-inc/pi-core";
import * as sdk from "openai";

interface FakeEndpoint {
	create(params: unknown, options: unknown): PromiseLike<unknown> & { withResponse?(): Promise<unknown> };
}

interface FakeClient {
	chat: { completions: FakeEndpoint };
	responses: FakeEndpoint;
}

type FakeOpenAI = new (options: unknown) => FakeClient;

/** A class the SDK module exports; a mock that does not define it has none (accessing it would throw). */
function sdkClass(name: "default" | "AzureOpenAI"): FakeOpenAI | undefined {
	return name in sdk ? (sdk as unknown as Record<string, FakeOpenAI>)[name] : undefined;
}

const OpenAI = sdkClass("default");
const AzureOpenAI = sdkClass("AzureOpenAI");

// The real SDK class carries its error classes as statics; a test's fake class does not.
const mocked = typeof (OpenAI as unknown as { APIError?: unknown } | undefined)?.APIError !== "function";

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
		const parsed = new URL(url);
		const apiVersion = parsed.searchParams.get("api-version");
		parsed.searchParams.delete("api-version");
		// Like the SDK, the core appends the path to the base URL string, so a base URL with a query gets the path inside
		// that query (`/v1?custom=true%2Fresponses`).
		const target = apiVersion === null ? url : decodeURIComponent(parsed.href);
		const endpoint = ENDPOINTS.find(([path]) => target.endsWith(path));
		if (!endpoint) return realFetch(url, init);
		const [path, select] = endpoint;
		const headers = (init.headers ?? {}) as Record<string, string>;
		const header = (wanted: string) => Object.entries(headers).find(([name]) => name.toLowerCase() === wanted)?.[1];
		const authorization = header("authorization");
		const defaultHeaders: Record<string, string | null> = Object.fromEntries(
			Object.entries(headers).filter(
				([name, value]) =>
					!SDK_HEADERS.test(name) &&
					!["authorization", "api-key"].includes(name.toLowerCase()) &&
					!(name.toLowerCase() === "user-agent" && /^(Azure)?OpenAI\/JS/.test(value)),
			),
		);
		const baseURL = target.slice(0, -path.length);
		let client: FakeClient;
		if (apiVersion !== null && AzureOpenAI) {
			// The TS sets the Azure client's User-Agent as `defaultHeaders["User-Agent"]`; the core sends names in lower case.
			const { "user-agent": userAgent, ...rest } = defaultHeaders;
			const azureHeaders = userAgent === undefined ? rest : { "User-Agent": userAgent, ...rest };
			client = new AzureOpenAI({
				apiKey: header("api-key"),
				apiVersion,
				dangerouslyAllowBrowser: true,
				defaultHeaders: azureHeaders,
				baseURL,
			});
		} else {
			defaultHeaders.Authorization = authorization ?? null;
			client = new (OpenAI as FakeOpenAI)({
				apiKey: authorization?.replace(/^Bearer /, ""),
				baseURL,
				dangerouslyAllowBrowser: true,
				defaultHeaders,
			});
		}
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

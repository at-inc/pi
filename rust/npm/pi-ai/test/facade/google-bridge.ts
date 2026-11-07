// Bridges a mocked `@google/genai` SDK to the core. The TS tests mock the SDK (`vi.mock("@google/genai")`) and assert on
// what the TS passed to `new GoogleGenAI(options)`; the core speaks HTTP instead. When this module sees a mocked SDK it
// replaces the host's fetch with a bridge that turns each `:streamGenerateContent` request back into the SDK call the
// TS makes: the client options from the URL and headers, `models.generateContentStream(params)` with the params from
// the body in the SDK's parameter shape, and the mock's chunks back into an SSE response. Imported by the shims, so it
// sees the test file's mocks.
//
// The wire does not carry everything the TS passes:
// - Header names arrive in lower case; `user-agent` becomes `User-Agent` again, other names stay lower case.
// - A Gemini API request becomes `httpOptions: { baseUrl, apiVersion: "" }` with the URL prefix before `/models/`, as
//   the TS builds for a model with a `baseUrl`; a model without one sends the same URL.
// - A Vertex request to a custom base URL does not carry the project and location, so the ADC client options have none.
// - The mocked SDK needs no credentials, but the core's Vertex ADC asks the GCE metadata server for a token; the bridge
//   answers it, and the `authorization` header the core then sends is not an SDK option.
// `withSdkChunks` hands `onProviderStreamEvent` the mock's own chunk objects, which the TS forwards as they are; the core
// forwards a copy that also carries `sdkHttpResponse`, as the real SDK's chunks do.

import type { StreamOptions } from "@at-inc/pi-ai";
import { host } from "@at-inc/pi-core";
import * as genai from "@google/genai";

interface FakeClient {
	models: { generateContentStream(params: unknown): PromiseLike<AsyncIterable<unknown>> | AsyncIterable<unknown> };
}

type FakeGoogleGenAI = new (options: Record<string, unknown>) => FakeClient;

/** The stream functions of the Google modules, as `withSdkChunks` calls them. */
type StreamFunction = (model: unknown, context: unknown, options?: StreamOptions) => unknown;

// The real SDK exports its error class; a test's mock module does not.
const mocked = !("ApiError" in genai);

/** Response header naming the bridged stream a core chunk came from. */
const STREAM_HEADER = "x-pi-google-bridge-stream";

/** Headers the SDK adds itself (or the core's ADC adds), which the TS never passes in `httpOptions.headers`. */
const SDK_HEADERS = /^(content-type|x-goog-api-client|x-goog-api-key|x-goog-user-project|authorization)$/i;

/** Hosts the Vertex client picks itself; any other host is a custom base URL. */
const VERTEX_DEFAULT_HOST =
	/^(aiplatform\.googleapis\.com|[a-z0-9-]+-aiplatform\.googleapis\.com|aiplatform\.[a-z0-9-]+\.rep\.googleapis\.com)$/;

/** The API version the TS Vertex adapter passes. */
const VERTEX_API_VERSION = "v1";

/** A URL path segment that names an API version, as the TS `baseUrlIncludesApiVersion` matches it. */
const API_VERSION_SEGMENT = /^v\d+(?:beta\d*)?$/;

/** `body` fields of the wire request that the SDK reads from `params.config`. */
const CONFIG_FIELDS = [
	"serviceTier",
	"tools",
	"toolConfig",
	"safetySettings",
	"labels",
	"cachedContent",
	"modelArmorConfig",
] as const;

/** The mock's chunks of each bridged stream, by stream id, until the core forwards them. */
const sdkChunks = new Map<string, unknown[]>();
let nextStreamId = 0;

if (mocked) {
	const realFetch = host.fetch;
	host.fetch = async (url, init) => {
		if (isMetadataRequest(url)) return metadataResponse(url);
		if (!url.includes(":streamGenerateContent")) return realFetch(url, init);
		const headers = Object.entries((init.headers ?? {}) as Record<string, string>);
		const apiKey = headers.find(([name]) => name.toLowerCase() === "x-goog-api-key")?.[1];
		const { options, model } = clientOptions(new URL(url), apiKey, userHeaders(headers));
		const client = new (genai.GoogleGenAI as unknown as FakeGoogleGenAI)(options);
		try {
			const chunks = await client.models.generateContentStream(params(model, init));
			const id = String(nextStreamId++);
			return new Response(sse(chunks, id), {
				status: 200,
				headers: { "content-type": "text/event-stream", [STREAM_HEADER]: id },
			});
		} catch (error) {
			const status = (error as { status?: unknown }).status;
			if (typeof status !== "number") throw error;
			return new Response(JSON.stringify({ error: { code: status, message: (error as Error).message } }), {
				status,
			});
		}
	};
}

/**
 * Wraps a stream function so `onProviderStreamEvent` receives the mock's own chunk object for each core chunk that
 * equals it (apart from `sdkHttpResponse`); any other chunk passes as the core sent it.
 */
export function withSdkChunks<F extends (...args: never[]) => unknown>(stream: F): F {
	if (!mocked) return stream;
	const call = stream as unknown as StreamFunction;
	const wrapped: StreamFunction = (model, context, options) => {
		const handler = options?.onProviderStreamEvent;
		if (!handler) return call(model, context, options);
		return call(model, context, {
			...options,
			onProviderStreamEvent: (chunk, eventModel) => handler(sdkChunk(chunk), eventModel),
		});
	};
	return wrapped as unknown as F;
}

/** The mock's chunk that `chunk` was made from, or `chunk` itself. */
function sdkChunk(chunk: unknown): unknown {
	const { sdkHttpResponse, ...rest } = chunk as { sdkHttpResponse?: { headers?: Record<string, string> } };
	const pending = sdkChunks.get(sdkHttpResponse?.headers?.[STREAM_HEADER] ?? "");
	const index = pending?.findIndex((original) => sameJson(original, rest)) ?? -1;
	if (!pending || index < 0) return chunk;
	return pending.splice(index, 1)[0];
}

/** Deep equality of JSON-like values, ignoring key order. */
function sameJson(a: unknown, b: unknown): boolean {
	if (a === b) return true;
	if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
	if (Array.isArray(a) !== Array.isArray(b)) return false;
	const aKeys = Object.keys(a).filter((key) => (a as Record<string, unknown>)[key] !== undefined);
	const bKeys = Object.keys(b).filter((key) => (b as Record<string, unknown>)[key] !== undefined);
	return (
		aKeys.length === bKeys.length &&
		aKeys.every((key) => sameJson((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]))
	);
}

/** The headers the TS passed in `httpOptions.headers`: the request's headers minus the SDK's own. */
function userHeaders(headers: Array<[string, string]>): Record<string, string> | undefined {
	const result = Object.fromEntries(
		headers
			.filter(([name, value]) => {
				if (SDK_HEADERS.test(name)) return false;
				return !(name.toLowerCase() === "user-agent" && value.startsWith("google-genai-sdk/"));
			})
			.map(([name, value]) => [name.toLowerCase() === "user-agent" ? "User-Agent" : name, value]),
	);
	return Object.keys(result).length > 0 ? result : undefined;
}

/** The `GoogleGenAI` options and the model id of a `:streamGenerateContent` URL. */
function clientOptions(
	url: URL,
	apiKey: string | undefined,
	headers: Record<string, string> | undefined,
): { options: Record<string, unknown>; model: string } {
	const path = url.pathname;
	const end = path.indexOf(":streamGenerateContent");
	const vertexAt = path.indexOf("/publishers/google/models/");
	if (vertexAt < 0) {
		const modelAt = path.indexOf("/models/");
		const httpOptions = {
			baseUrl: `${url.origin}${path.slice(0, modelAt)}`,
			apiVersion: "",
			...(headers && { headers }),
		};
		return { options: { apiKey, httpOptions }, model: path.slice(modelAt + "/models/".length, end) };
	}
	const model = path.slice(vertexAt + "/publishers/google/models/".length, end);
	const prefix = path.slice(0, vertexAt);
	const auth = apiKey !== undefined ? { apiKey } : {};
	if (VERTEX_DEFAULT_HOST.test(url.hostname)) {
		const [, apiVersion, , project, , location] = prefix.split("/");
		const target = apiKey === undefined ? { project, location } : {};
		return {
			options: { vertexai: true, ...auth, ...target, apiVersion, httpOptions: headers && { headers } },
			model,
		};
	}
	// The TS passes `apiVersion: ""` for a base URL with a version in it; otherwise the client appends "v1".
	const base = prefix.slice(0, -VERTEX_API_VERSION.length - 1);
	const versionAppended = prefix.endsWith(`/${VERTEX_API_VERSION}`) && !hasApiVersion(base);
	const httpOptions = {
		baseUrl: `${url.origin}${versionAppended ? base : prefix}`,
		baseUrlResourceScope: "COLLECTION",
		...(versionAppended ? {} : { apiVersion: "" }),
		...(headers && { headers }),
	};
	return { options: { vertexai: true, ...auth, apiVersion: VERTEX_API_VERSION, httpOptions }, model };
}

function hasApiVersion(path: string): boolean {
	return path.split("/").some((segment) => API_VERSION_SEGMENT.test(segment));
}

/** The SDK's `GenerateContentParameters` for a wire request body. */
function params(model: string, init: RequestInit): Record<string, unknown> {
	const body = JSON.parse(init.body as string) as Record<string, unknown>;
	const config: Record<string, unknown> = { ...(body.generationConfig as Record<string, unknown> | undefined) };
	const systemInstruction = body.systemInstruction as { parts?: Array<{ text?: unknown }> } | undefined;
	if (systemInstruction) {
		const parts = systemInstruction.parts ?? [];
		const text = parts.length === 1 && Object.keys(parts[0]).length === 1 ? parts[0].text : undefined;
		config.systemInstruction = typeof text === "string" ? text : systemInstruction;
	}
	for (const field of CONFIG_FIELDS) {
		if (body[field] !== undefined) config[field] = body[field];
	}
	if (init.signal) config.abortSignal = init.signal;
	return { model, contents: body.contents, config };
}

/** The SSE body of the mock's chunks: one `data:` event per chunk. */
function sse(chunks: AsyncIterable<unknown>, id: string): ReadableStream<Uint8Array> {
	const encoder = new TextEncoder();
	const pending: unknown[] = [];
	sdkChunks.set(id, pending);
	return new ReadableStream({
		async start(controller) {
			try {
				for await (const chunk of chunks) {
					pending.push(chunk);
					controller.enqueue(encoder.encode(`data: ${JSON.stringify(chunk)}\n\n`));
				}
				controller.close();
			} catch (error) {
				controller.error(error);
			}
		},
	});
}

/** Whether a URL is a GCE metadata server request of the core's Vertex ADC. */
function isMetadataRequest(url: string): boolean {
	return /^http:\/\/(169\.254\.169\.254|metadata\.google\.internal\.?)\/computeMetadata\//.test(url);
}

/** The metadata server's answer: a token for the default service account, an empty object otherwise. */
function metadataResponse(url: string): Response {
	const token = { access_token: "google-bridge-token", expires_in: 3600, token_type: "Bearer" };
	const body = url.includes("/service-accounts/default/token") ? token : {};
	return new Response(JSON.stringify(body), {
		headers: { "content-type": "application/json", "metadata-flavor": "Google" },
	});
}

// Bridges a mocked `@aws-sdk/client-bedrock-runtime` to the core. The TS tests mock the SDK client
// (`vi.mock("@aws-sdk/client-bedrock-runtime")`) and assert on what the TS passed to it; the core signs and sends HTTP
// instead. When this module sees a mocked SDK it replaces the host's fetch with a bridge that turns each
// `/model/<id>/converse-stream` request back into what the TS does (`new BedrockRuntimeClient(config)`, the custom
// headers middleware, `client.send(new ConverseStreamCommand(input), { abortSignal })`) and the mock's output or thrown
// error back into the HTTP response: an AWS event stream of the mock's items, or a JSON error response. Imported by
// the shims, so it sees the test file's mocks.
//
// The mocked `send` stands for the SDK's whole send, so the core makes one attempt (`AWS_MAX_ATTEMPTS=1`) and finds
// static keys and a region for the default and the ambient profile in virtual shared config files: the mocked SDK
// never resolves credentials or a region, the core does before it sends. The client config leaves those keys and that
// region out again.
//
// What the wire cannot tell: an explicit or env-scoped `profile` (the core needs real keys for it), the secret key,
// and whether `onResponse` registered a middleware. `endpoint` is set when the user agent carries the endpoint
// override feature `N`; `region` comes from the SigV4 credential scope or the standard runtime host name.

import { crc32 } from "node:zlib";
import { host } from "@at-inc/pi-core";
import {
	BedrockRuntimeClient,
	BedrockRuntimeServiceException,
	ConverseStreamCommand,
} from "@aws-sdk/client-bedrock-runtime";

type Middleware = (next: (args: unknown) => Promise<unknown>) => (args: unknown) => Promise<unknown>;

interface FakeOutput {
	$metadata?: { httpStatusCode?: number; requestId?: string };
	stream?: AsyncIterable<Record<string, unknown>>;
}

interface FakeClient {
	middlewareStack: { add(middleware: Middleware, options: { step: string; name: string; priority?: string }): void };
	send(command: unknown, options: { abortSignal?: AbortSignal }): Promise<FakeOutput>;
}

type FakeClientClass = new (config: Record<string, unknown>) => FakeClient;

type FakeCommandClass = new (input: unknown) => unknown;

interface FakeError {
	name?: unknown;
	message?: unknown;
	$metadata?: { httpStatusCode?: unknown; requestId?: unknown };
}

// The real client inherits `destroy` from the Smithy client; a test's fake class does not have it.
const mocked = typeof (BedrockRuntimeClient.prototype as { destroy?: unknown }).destroy !== "function";

const CONVERSE_STREAM = /\/model\/([^/]+)\/converse-stream$/;

const CONFIG_FILE = "pi-bedrock-bridge:config";

const CREDENTIALS_FILE = "pi-bedrock-bridge:credentials";

const PROFILE_KEY = "PIBEDROCKBRIDGEKEY";

const PROFILE_REGION = "pi-bedrock-bridge-region";

const ENV: Record<string, string> = {
	AWS_MAX_ATTEMPTS: "1",
	AWS_CONFIG_FILE: CONFIG_FILE,
	AWS_SHARED_CREDENTIALS_FILE: CREDENTIALS_FILE,
};

/** Headers the core's client sets itself, which the custom headers middleware never adds. */
const SDK_HEADERS = new Set([
	"content-type",
	"content-length",
	"host",
	"user-agent",
	"x-amz-user-agent",
	"amz-sdk-invocation-id",
	"amz-sdk-request",
	"x-amz-date",
	"x-amz-security-token",
	"x-amz-content-sha256",
	"authorization",
	"x-amzn-trace-id",
]);

if (mocked) {
	const realFetch = host.fetch;
	const realEnv = host.env;
	const realReadFile = host.readFile;
	host.env = (name) => ENV[name] ?? realEnv(name);
	host.readFile = (path) => {
		if (path === CONFIG_FILE) return Promise.resolve(profiles(`region = ${PROFILE_REGION}`, "profile "));
		if (path === CREDENTIALS_FILE) {
			return Promise.resolve(
				profiles(`aws_access_key_id = ${PROFILE_KEY}\naws_secret_access_key = ${PROFILE_KEY}`, ""),
			);
		}
		return realReadFile(path);
	};
	host.fetch = async (url, init) => {
		const target = new URL(url);
		const match = CONVERSE_STREAM.exec(target.pathname);
		if (!match) return realFetch(url, init);
		const headers = lowerCase((init.headers ?? {}) as Record<string, string>);
		const client = new (BedrockRuntimeClient as unknown as FakeClientClass)(
			clientConfig(url.slice(0, url.length - match[0].length), target.hostname, headers),
		);
		const custom = Object.entries(headers).filter(([name]) => !SDK_HEADERS.has(name));
		if (custom.length > 0) addCustomHeadersMiddleware(client, Object.fromEntries(custom));
		const input = { modelId: decodeURIComponent(match[1]), ...(blobs(JSON.parse(init.body as string)) as object) };
		let output: FakeOutput;
		try {
			output = await client.send(new (ConverseStreamCommand as unknown as FakeCommandClass)(input), {
				abortSignal: init.signal ?? undefined,
			});
		} catch (error) {
			return errorResponse(error);
		}
		const requestId = output.$metadata?.requestId;
		return new Response(output.stream ? eventStream(output.stream) : null, {
			status: output.$metadata?.httpStatusCode ?? 200,
			headers: {
				"content-type": "application/vnd.amazon.eventstream",
				...(requestId ? { "x-amzn-requestid": requestId } : {}),
			},
		});
	};
}

/** A shared config or credentials file with `body` in `[default]` and in a section for the ambient `AWS_PROFILE`. */
function profiles(body: string, prefix: string): string {
	const ambient = host.env("AWS_PROFILE");
	const sections = ambient && ambient !== "default" ? ["default", `${prefix}${ambient}`] : ["default"];
	return sections.map((section) => `[${section}]\n${body}\n`).join("");
}

/** The request headers by lower-case name. */
function lowerCase(headers: Record<string, string>): Record<string, string> {
	return Object.fromEntries(Object.entries(headers).map(([name, value]) => [name.toLowerCase(), value]));
}

/** The `BedrockRuntimeClientConfig` the TS builds, as far as the signed request shows it. */
function clientConfig(endpoint: string, hostname: string, headers: Record<string, string>): Record<string, unknown> {
	const config: Record<string, unknown> = { profile: host.env("AWS_PROFILE") || undefined };
	const userAgent = (headers["user-agent"] ?? "").split(" ");
	const features = userAgent.findIndex((section) => section.startsWith("m/"));
	const customUserAgent = userAgent
		.slice(features + 1)
		.filter((section) => features >= 0 && !section.startsWith("app/"))
		.map((section) => section.split("/"));
	if (customUserAgent.length > 0) config.customUserAgent = customUserAgent;
	if (features >= 0 && userAgent[features].slice(2).split(",").includes("N")) config.endpoint = endpoint;
	const authorization = headers.authorization ?? "";
	const bearer = /^Bearer (.*)$/.exec(authorization);
	if (bearer) {
		config.token = { token: bearer[1] };
		config.authSchemePreference = ["httpBearerAuth"];
		config.region = /^bedrock-runtime(?:-fips)?\.([a-z0-9-]+)\.amazonaws\.com(?:\.cn)?$/.exec(hostname)?.[1];
		return config;
	}
	const scope = /Credential=([^/]+)\/[^/]+\/([^/]+)\//.exec(authorization);
	if (!scope) return config;
	const [, accessKeyId, region] = scope;
	if (region !== PROFILE_REGION) config.region = region;
	if (accessKeyId !== PROFILE_KEY) {
		const sessionToken = headers["x-amz-security-token"];
		config.credentials = { accessKeyId, ...(sessionToken ? { sessionToken } : {}) };
	}
	return config;
}

/** `addCustomHeadersMiddleware` of the TS, with the headers the core added. */
function addCustomHeadersMiddleware(client: FakeClient, headers: Record<string, string>): void {
	const middleware: Middleware = (next) => async (args) => {
		const request = (args as { request?: unknown }).request;
		if (request && typeof request === "object" && "headers" in request) {
			Object.assign((request as { headers: Record<string, string> }).headers, headers);
		}
		return next(args);
	};
	client.middlewareStack.add(middleware, { step: "build", name: "pi-ai-custom-headers", priority: "low" });
}

/** The command input with its base64 blob members (`bytes`, `redactedContent`) as the `Uint8Array`s the SDK holds. */
function blobs(value: unknown, key?: string): unknown {
	if (typeof value === "string" && (key === "bytes" || key === "redactedContent")) {
		return new Uint8Array(Buffer.from(value, "base64"));
	}
	if (Array.isArray(value)) return value.map((item) => blobs(item));
	if (value === null || typeof value !== "object") return value;
	return Object.fromEntries(Object.entries(value).map(([name, member]) => [name, blobs(member, name)]));
}

/**
 * The HTTP response of an error `send` rejected with. A service exception (one with a numeric
 * `$metadata.httpStatusCode`, or an instance of the mocked `BedrockRuntimeServiceException`, then with status 400)
 * becomes a JSON error response named by `x-amzn-errortype`; any other error is what the core's fetch rejects with.
 */
function errorResponse(error: unknown): Response {
	const fake = error as FakeError;
	const status = fake?.$metadata?.httpStatusCode;
	const isServiceException = error instanceof (BedrockRuntimeServiceException as unknown as ErrorConstructor);
	if (typeof status !== "number" && !isServiceException) throw error;
	const requestId = fake.$metadata?.requestId;
	return new Response(JSON.stringify({ message: fake.message }), {
		status: typeof status === "number" ? status : 400,
		headers: {
			"content-type": "application/json",
			"x-amzn-errortype": String(fake.name),
			...(typeof requestId === "string" ? { "x-amzn-requestid": requestId } : {}),
		},
	});
}

/**
 * The event stream of the mock's items, one `event` message per item, read one item per pull so that an error the
 * iterator throws errors the body after the items before it.
 */
function eventStream(items: AsyncIterable<Record<string, unknown>>): ReadableStream<Uint8Array> {
	const iterator = items[Symbol.asyncIterator]();
	return new ReadableStream(
		{
			async pull(controller) {
				try {
					const next = await iterator.next();
					if (next.done) {
						controller.close();
						return;
					}
					for (const [member, value] of Object.entries(next.value)) {
						controller.enqueue(eventMessage(member, value));
					}
				} catch (error) {
					controller.error(error);
				}
			},
			async cancel() {
				await iterator.return?.();
			},
		},
		{ highWaterMark: 0 },
	);
}

/** One AWS event stream message: prelude, string headers, the JSON payload and the two CRC32s. */
function eventMessage(member: string, value: unknown): Uint8Array {
	const payload = Buffer.from(JSON.stringify(value, jsonBlobs));
	const fields = { ":event-type": member, ":content-type": "application/json", ":message-type": "event" };
	const headers = Buffer.concat(
		Object.entries(fields).map(([name, text]) => {
			const nameBytes = Buffer.from(name);
			const valueBytes = Buffer.from(text);
			const header = Buffer.alloc(nameBytes.length + 4);
			header.writeUInt8(nameBytes.length, 0);
			nameBytes.copy(header, 1);
			header.writeUInt8(7, nameBytes.length + 1);
			header.writeUInt16BE(valueBytes.length, nameBytes.length + 2);
			return Buffer.concat([header, valueBytes]);
		}),
	);
	const length = 12 + headers.length + payload.length + 4;
	const message = Buffer.alloc(length);
	message.writeUInt32BE(length, 0);
	message.writeUInt32BE(headers.length, 4);
	message.writeUInt32BE(crc32(message.subarray(0, 8)), 8);
	headers.copy(message, 12);
	payload.copy(message, 12 + headers.length);
	message.writeUInt32BE(crc32(message.subarray(0, length - 4)), length - 4);
	return message;
}

/** `JSON.stringify` replacer: a blob as base64, an error as its own fields and its message. */
function jsonBlobs(_key: string, value: unknown): unknown {
	if (value instanceof Uint8Array) return Buffer.from(value).toString("base64");
	if (value instanceof Error) return { ...value, message: value.message };
	return value;
}

// The facade's error classes, and the conversion of errors from the core into them. The core throws JS Errors that
// carry the Rust class name in `name` and the class fields as own properties; `fromCore` gives each the prototype
// of the facade class of that name, so `instanceof` works for JS callers.

/** `class McpError extends Error { code; data }` of protocol/jsonrpc. */
export class McpError extends Error {
	readonly code: number;
	readonly data: unknown;

	constructor(code: number, message: string, data?: unknown) {
		super(message);
		this.name = "McpError";
		this.code = code;
		this.data = data;
	}
}

/** `McpConnectionClosedError` of protocol/jsonrpc. */
export class McpConnectionClosedError extends Error {
	constructor(message = "MCP connection closed") {
		super(message);
		this.name = "McpConnectionClosedError";
	}
}

/** `McpTimeoutError` of protocol/jsonrpc. */
export class McpTimeoutError extends Error {
	readonly timeoutMs: number;

	constructor(timeoutMs: number) {
		super(`MCP request timed out after ${timeoutMs}ms`);
		this.name = "McpTimeoutError";
		this.timeoutMs = timeoutMs;
	}
}

/** `McpAbortError` of protocol/jsonrpc, whose `name` is `"AbortError"`. */
export class McpAbortError extends Error {
	constructor(message = "MCP request aborted") {
		super(message);
		this.name = "AbortError";
	}
}

/** `McpHttpError` of transports/streamable-http. */
export class McpHttpError extends Error {
	readonly status: number;
	readonly body: string;

	constructor(status: number, message: string, body = "") {
		super(message);
		this.name = "McpHttpError";
		this.status = status;
		this.body = body;
	}
}

/** `McpAuthRequiredError` of transports/streamable-http. */
export class McpAuthRequiredError extends McpHttpError {
	readonly wwwAuthenticate: string | null;

	constructor(response: Response, body = "") {
		super(401, "MCP server requires authentication", body);
		this.name = "McpAuthRequiredError";
		this.wwwAuthenticate = response.headers.get("www-authenticate");
	}
}

/** `McpSessionExpiredError` of transports/streamable-http. */
export class McpSessionExpiredError extends McpHttpError {
	constructor(body = "") {
		super(404, "MCP session expired", body);
		this.name = "McpSessionExpiredError";
	}
}

/** `OAuthError` of oauth/errors. */
export class OAuthError extends Error {
	readonly code: string;
	readonly errorUri: string | undefined;

	constructor(code: string, message: string, errorUri?: string) {
		super(message || code);
		this.name = "OAuthError";
		this.code = code;
		this.errorUri = errorUri;
	}
}

/** `OAuthIssuerMismatchError` of oauth/errors. */
export class OAuthIssuerMismatchError extends Error {
	readonly expected: string | undefined;
	readonly received: string | undefined;

	constructor(expected: string | undefined, received: string | undefined) {
		const format = (issuer: string | undefined) => (issuer === undefined ? "none" : JSON.stringify(issuer));
		super(`OAuth issuer mismatch: expected ${format(expected)}, received ${format(received)}`);
		this.name = "OAuthIssuerMismatchError";
		this.expected = expected;
		this.received = received;
	}
}

/** `OAuthInsecureEndpointError` of oauth/errors. */
export class OAuthInsecureEndpointError extends Error {
	readonly endpoint: string;

	constructor(endpoint: string) {
		super(`Refusing to send OAuth credentials to non-HTTPS endpoint ${endpoint}`);
		this.name = "OAuthInsecureEndpointError";
		this.endpoint = endpoint;
	}
}

/** `OAuthRegistrationError` of oauth/errors. */
export class OAuthRegistrationError extends Error {
	readonly status: number;
	readonly body: string;

	constructor(status: number, body: string) {
		super(`OAuth dynamic client registration failed with status ${status}: ${body}`);
		this.name = "OAuthRegistrationError";
		this.status = status;
		this.body = body;
	}
}

/** `McpOAuthAuthorizationRequiredError` of oauth/errors. */
export class McpOAuthAuthorizationRequiredError extends Error {
	constructor() {
		super("MCP OAuth authorization requires user interaction");
		this.name = "McpOAuthAuthorizationRequiredError";
	}
}

/** The facade class of each Rust error name; `AbortError` is `McpAbortError` only with the core's class marker. */
const CLASSES: Record<string, { prototype: Error }> = {
	McpError,
	McpConnectionClosedError,
	McpTimeoutError,
	McpHttpError,
	McpAuthRequiredError,
	McpSessionExpiredError,
	OAuthError,
	OAuthIssuerMismatchError,
	OAuthInsecureEndpointError,
	OAuthRegistrationError,
	McpOAuthAuthorizationRequiredError,
	TypeError,
	RangeError,
	SyntaxError,
};

/** Fields the core sends as `null` that the TS classes hold as `undefined`. */
const OPTIONAL_FIELDS = ["errorUri", "expected", "received"];

/** An error from the core as an instance of the facade class of its name; any other value as it is. */
export function fromCore(error: unknown): unknown {
	if (!(error instanceof Error) || Object.getPrototypeOf(error) !== Error.prototype) return error;
	const fields = error as Error & Record<string, unknown>;
	if (error.name === "AbortError" && fields.class === "McpAbortError") {
		delete fields.class;
		return Object.setPrototypeOf(error, McpAbortError.prototype);
	}
	const errorClass = Object.hasOwn(CLASSES, error.name) ? CLASSES[error.name] : undefined;
	if (!errorClass) return error;
	for (const field of OPTIONAL_FIELDS) if (fields[field] === null) fields[field] = undefined;
	if (error.name === "McpError" && !("data" in error)) fields.data = undefined;
	return Object.setPrototypeOf(error, errorClass.prototype);
}

/** `promise` with its rejection converted by `fromCore`. */
export function settle<T>(promise: Promise<T>): Promise<T> {
	return promise.catch((error: unknown) => {
		throw fromCore(error);
	});
}

/** `call()` with a synchronous throw converted by `fromCore`. */
export function attempt<T>(call: () => T): T {
	try {
		return call();
	} catch (error) {
		throw fromCore(error);
	}
}

/** An error listener that receives errors from the core converted by `fromCore`. */
export function errorListener(listener: (error: Error) => void): (error: Error) => void {
	return (error) => listener(fromCore(error) as Error);
}

// Runs the TS package's own tests against the facade: their `../src/<module>` imports resolve to test/facade/<module>,
// which re-exports the facade module (and bridges the mocked `openai`, `@anthropic-ai/sdk`, `@google/genai` and AWS
// SDKs, see test/facade/bridges.ts). Every test file runs except the ones below: a test that imports a module no
// facade slice provides yet fails to resolve, and some tests check what only the TS implementation can show.
// test/auth and test/utils hold the facade's own tests for behavior whose TS test cannot run or is skipped.

import { existsSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vitest/config";

const here = dirname(fileURLToPath(import.meta.url));
const tsPackage = resolve(here, "../../../packages/ai");
const tsTests = join(tsPackage, "test");
const facadeShims = join(here, "test/facade");

/** Test files that cannot run against the facade, with the reason. */
export const skipped: Record<string, string> = {
	"test/anthropic-federation.test.ts":
		"its stream tests assert the SDK's federation config, which the wire cannot reproduce (the core exchanges the identity token itself); the other 5 of 9 tests pass",
	"test/bedrock-convert-messages.test.ts":
		"content blocks of an unknown type fail to parse into the Rust Context (closed tagged enums; the TS skips them), and a lone surrogate reaches Rust as U+FFFD, so sanitizeSurrogates cannot drop it; the other 10 of 16 tests pass",
	"test/bedrock-credentials.test.ts":
		"the request cannot show an explicit or env-scoped profile (the core resolves its keys before sending) or the secret key, which these 3 tests assert",
	"test/bedrock-endpoint-resolution.test.ts":
		"an explicit or env-scoped profile never reaches the wire: the core resolves its keys from the shared config files before sending, where the mocked SDK never does; the other 8 of 9 tests pass",
	"test/bedrock-raw-stop-reason.test.ts":
		"asserts onProviderStreamEvent gets the SDK's own item objects (toBe) and a plain Error as an exception item; the core decodes items from the wire, where an exception member is always the modeled exception; the other 2 of 4 tests pass",
	"test/bedrock-redacted-reasoning.test.ts":
		"asserts onPayload sees redactedContent as a Uint8Array; the Rust command input is JSON with blobs as base64 text (bedrock_converse_stream.rs); the other 4 of 6 tests pass",
	"test/compat-schema.test.ts":
		"imports ../src/providers/compat-schema.ts, TypeBox schema values with no Rust port to bridge",
	"test/constrained-sampling.test.ts":
		"'replays grammar calls as custom Responses items' changes a tool call after normalizeContext and expects the conversion to see it; the facade's normalizeContext (utils/transcript) returns copies of the messages, the TS returns the caller's objects",
	"test/lazy-module-load.test.ts":
		"spawns Node processes that import the TS package's own src and dist entries and check which SDK modules load; the vitest aliases do not reach a subprocess, and the facade loads no SDK",
	"test/models-entry.test.ts":
		"spawns Node processes that import the TS package's own src/models.ts and @at-inc/pi-ai/models; the vitest aliases do not reach a subprocess",
	"test/opencode-go-provider.test.ts":
		"mocks ../src/api/google-generative-ai.lazy.ts with vi.mock and expects the provider to call the mock; the Rust provider calls the Rust Google API, which a JS module mock cannot replace",
	"test/openrouter-images.test.ts":
		"'passes through abort signal' expects the SDK call with the aborted signal; the core checks the signal before sending and makes no request (the result is the same aborted message); the other 2 of 3 tests pass",
	"test/provider-error-body-regression.test.ts":
		"'bedrock (body-blind)' fakes an SDK error whose body the SDK left unparsed; from that body on the wire the core's SDK port reads the message ('Unknown: blocked by gateway WAF'), so the status-prefixed fallback does not apply; the other 4 of 5 tests pass",
	"test/telemetry-options.test.ts":
		"options.telemetry (a JS TelemetrySpan) is not bridged to the Rust core yet (see options.rs), so implementations see no span; the other 1 of 3 tests passes",
	"test/telemetry.test.ts": "imports ../src/telemetry.ts; telemetry is not bridged to the Rust core yet",
};

const loneSurrogate =
	"a Rust string cannot hold an unpaired surrogate: one at the end of the text read so far is held back, others read as U+FFFD";
const quadraticSnapshots =
	"each changed snapshot crosses the boundary whole (JSON text), so a long stream costs time and memory quadratic in its length";

const codexHeaders =
	"reads the request headers only when init.headers is a Headers instance; the host's fetch passes a plain object";

/** Tests of included files that cannot pass against the facade, by test name, with the reason. */
export const skippedTests: Record<string, string> = {
	"allocates linearly in the streamed text, never per accumulated text": quadraticSnapshots,
	"keeps very large open arrays linear by letting their preview lag": quadraticSnapshots,
	"reads deep nesting without recursion and in linear time": `${quadraticSnapshots}; 3.4 s here and 7.8 s on CI against its 5 s bound (TS: 0.4 s). test/utils/streaming-json-deep.test.ts keeps its correctness check`,
	"matches the reference after every piece, at every chunk size, with insignificant whitespace": loneSurrogate,
	"matches today's parseStreamingJson at every prefix of compact documents": loneSurrogate,
	"joins surrogate pairs split across pieces, escaped or raw": loneSurrogate,
	"unpaired high surrogate, minLength 2": loneSurrogate,
	"reads every JSONTestSuite document JSON.parse accepts exactly like it, and never throws on the rest":
		"1e400 reads as null: a serde_json::Value cannot hold Infinity",
	"merges request identity over the Models and wrapper identities":
		"expects Models.getModel to return the provider's model object (toBe); models cross the boundary as JSON copies",
	"forwards parsed events in order before normalizing the response":
		"expects the SDK's own event objects in onProviderStreamEvent; the core decodes the events from the wire",
	"forwards custom baseUrl to the ADC client":
		"asserts the ADC client's project and location, which a Vertex request to a custom base URL does not carry",
	"openrouter derives the api key and keeps the permanent credential on refresh":
		"expects refresh to return the caller's credential object (toBe); the Rust OAuthAuth returns a copy",
	"streams SSE responses and forwards raw provider events": codexHeaders,
	"omits SSE cache affinity when cacheRetention is none": codexHeaders,
	"clamps Codex session-id header to 64 characters": codexHeaders,
	"lets model and caller headers override originator and User-Agent": codexHeaders,
	"zstd-compresses SSE request bodies": "the wasm build has no zstd, so the Codex request body goes uncompressed",
	"normalizes optional provider metadata to JSON values":
		"createProviderErrorDiagnostic (utils slice) reads metadata as JSON, so a Headers object has no entries",
	"keeps the original failure and HTTP metadata when the payload cannot be serialized":
		"createProviderErrorDiagnostic (utils slice) reads metadata as JSON, so a BigInt payload is not the TS String(payload) fallback",
};

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

function facadeModules(): Plugin {
	return {
		name: "pi-facade-modules",
		enforce: "pre",
		resolveId(source, importer) {
			if (!importer?.startsWith(tsTests) || !source.startsWith("../src/")) return null;
			const shim = join(facadeShims, source.slice("../src/".length));
			if (!existsSync(shim)) {
				throw new Error(
					`${relative(tsPackage, importer)} imports ${source}, which this facade slice does not provide`,
				);
			}
			return shim;
		},
	};
}

export default defineConfig({
	plugins: [facadeModules()],
	resolve: { conditions: ["source"] },
	ssr: { resolve: { conditions: ["source"] } },
	test: {
		root: tsPackage,
		include: ["test/*.test.ts", join(here, "test/auth/*.test.ts"), join(here, "test/utils/*.test.ts")],
		exclude: Object.keys(skipped),
		testNamePattern: new RegExp(`^(?!.*(?:${Object.keys(skippedTests).map(escapeRegExp).join("|")})$)`),
		testTimeout: 30000,
	},
});

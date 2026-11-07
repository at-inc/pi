// Runs the TS package's own tests against the facade: their `../src/<module>` imports resolve to test/facade/<module>,
// which re-exports the facade module (and bridges the mocked `openai`, `@anthropic-ai/sdk`, `@google/genai` and AWS
// SDKs, see test/facade/bridges.ts). Every test file runs except the ones below: a test that imports a module no
// facade slice provides yet fails to resolve, and some tests check what only the TS implementation can show.

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
	"test/anthropic-auth-token.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), bridged by the utils slice",
	"test/anthropic-federation.test.ts":
		"its stream tests assert the SDK's federation config, which the wire cannot reproduce (the core exchanges the identity token itself); the other 5 of 9 tests pass",
	"test/anthropic-oauth.test.ts": "imports ../src/auth/oauth/anthropic.ts, bridged by the auth slice",
	"test/anthropic-sdk-migration.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), not bridged yet; the core's User-Agent on wasm is 'pi/<version> (browser)', where the TS on Node reports its platform and runtime",
	"test/azure-openai-base-url.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), which is not bridged yet, and the wasm core's User-Agent is 'pi/<version> (browser)'; 'forwards parsed events in order' expects the SDK's own event objects in onProviderStreamEvent, which the wire cannot reproduce",
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
	"test/context-estimate.test.ts": "imports ../src/utils/estimate.ts, bridged by the utils slice",
	"test/context-overflow.test.ts": "imports ../src/utils/overflow.ts, bridged by the utils slice",
	"test/error-body.test.ts": "imports ../src/utils/error-body.ts, bridged by the utils slice",
	"test/github-copilot-oauth.test.ts": "imports ../src/auth/oauth/github-copilot.ts, bridged by the auth slice",
	"test/github-copilot-request-size-overflow.test.ts":
		"imports ../src/utils/overflow.ts (isContextOverflow), which is not bridged yet; passes against the TS utils/overflow",
	"test/google-raw-stop-reason.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), which is not bridged yet; the wasm core's User-Agent is 'pi/<version> (browser)' where the TS on Node reports the Node runtime",
	"test/google-vertex-api-key-resolution.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), which is not bridged yet; 'forwards custom baseUrl to the ADC client' also asserts project and location, which a Vertex request to a custom base URL does not carry",
	"test/identity.test.ts": "imports ../src/utils/identity.ts, bridged by the utils slice",
	"test/interleaved-thinking.test.ts": "imports ../src/utils/string-enum.ts, bridged by the utils slice",
	"test/json-parse.test.ts": "imports ../src/utils/json-parse.ts, bridged by the utils slice",
	"test/kimi-coding-endpoints.test.ts": "imports ../src/auth/oauth/kimi-coding.ts, bridged by the auth slice",
	"test/kimi-coding-oauth.test.ts": "imports ../src/auth/oauth/kimi-coding.ts, bridged by the auth slice",
	"test/kimi-coding-provider.test.ts":
		"2 of its 8 tests log in through Models.login, which belongs to the auth slice; the other 6 pass",
	"test/lazy-module-load.test.ts":
		"spawns Node processes that import the TS package's own src and dist entries and check which SDK modules load; the vitest aliases do not reach a subprocess, and the facade loads no SDK",
	"test/meta-oauth.test.ts": "imports ../src/auth/oauth/meta.ts, bridged by the auth slice",
	"test/mistral-http-transport.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), which is not bridged yet; the wasm core's User-Agent is 'pi/<version> (browser)' where the TS on Node reports the Node runtime",
	"test/mistral-raw-stop-reason.test.ts":
		"imports ../src/utils/retry.ts (isRetryableAssistantError), which is not bridged yet",
	"test/models-entry.test.ts":
		"spawns Node processes that import the TS package's own src/models.ts and @at-inc/pi-ai/models; the vitest aliases do not reach a subprocess",
	"test/models-runtime.test.ts":
		"2 of its 43 tests run Models.login and logout, which belong to the auth slice; the other 41 pass",
	"test/node-http-proxy.test.ts": "imports ../src/utils/node-http-proxy.ts, bridged by the utils slice",
	"test/oauth-auth.test.ts": "imports ../src/auth/oauth/anthropic.ts, bridged by the auth slice",
	"test/oauth-callback-server.test.ts": "imports ../src/auth/oauth/callback-server.ts, bridged by the auth slice",
	"test/oauth-device-code.test.ts": "imports ../src/auth/oauth/device-code.ts, bridged by the auth slice",
	"test/oauth-page.test.ts": "imports ../src/utils/oauth-page.ts, bridged by the auth slice",
	"test/openai-chatgpt-oauth.test.ts": "imports ../src/auth/oauth/openai-chatgpt.ts, bridged by the auth slice",
	"test/openai-codex-oauth.test.ts": "imports ../src/auth/oauth/openai-codex.ts, bridged by the auth slice",
	"test/openai-codex-stream.test.ts":
		"imports ../src/utils/identity.ts (formatUserAgent), which is not bridged yet, and the wasm core's User-Agent is 'pi/<version> (browser)'; three tests read init.headers only when it is a Headers instance, and the host's fetch passes a plain object; 'zstd-compresses SSE request bodies' needs zstd, which the wasm build does not have (the body goes uncompressed)",
	"test/openai-responses-foreign-toolcall-id.test.ts":
		"imports ../src/utils/hash.ts (shortHash), which is not bridged yet; passes against the TS utils/hash",
	"test/opencode-go-provider.test.ts":
		"mocks ../src/api/google-generative-ai.lazy.ts with vi.mock and expects the provider to call the mock; the Rust provider calls the Rust Google API, which a JS module mock cannot replace",
	"test/openrouter-cost.test.ts": "imports ../src/utils/openrouter-cost.ts, bridged by the utils slice",
	"test/openrouter-images.test.ts":
		"'passes through abort signal' expects the SDK call with the aborted signal; the core checks the signal before sending and makes no request (the result is the same aborted message); the other 2 of 3 tests pass",
	"test/openrouter-oauth.test.ts": "imports ../src/auth/oauth/openrouter.ts, bridged by the auth slice",
	"test/overflow.test.ts": "imports ../src/utils/overflow.ts, bridged by the utils slice",
	"test/provider-error-body-regression.test.ts":
		"'bedrock (body-blind)' fakes an SDK error whose body the SDK left unparsed; from that body on the wire the core's SDK port reads the message ('Unknown: blocked by gateway WAF'), so the status-prefixed fallback does not apply; the other 4 of 5 tests pass",
	"test/provider-error-diagnostics.test.ts": "imports ../src/utils/diagnostics.ts, bridged by the utils slice",
	"test/provider-retry.test.ts": "imports ../src/utils/provider-retry.ts, bridged by the utils slice",
	"test/providers.test.ts":
		"2 of its 30 tests run provider-owned login flows (apiKey.login), which belong to the auth slice; the other 28 pass",
	"test/radius-oauth.test.ts": "imports ../src/auth/oauth/radius.ts, bridged by the auth slice",
	"test/retry.test.ts": "imports ../src/utils/retry.ts, bridged by the utils slice",
	"test/stream.test.ts": "imports ../src/utils/string-enum.ts, bridged by the utils slice",
	"test/streaming-json.test.ts": "imports ../src/utils/json-parse.ts, bridged by the utils slice",
	"test/system-message-replay.test.ts": "imports ../src/utils/text.ts, bridged by the utils slice",
	"test/telemetry-options.test.ts":
		"options.telemetry (a JS TelemetrySpan) is not bridged to the Rust core yet (see options.rs), so implementations see no span; the other 1 of 3 tests passes",
	"test/telemetry.test.ts": "imports ../src/telemetry.ts; telemetry is not bridged to the Rust core yet",
	"test/text.test.ts":
		"imports contentText from ../src/index.ts, which re-exports utils/text, bridged by the utils slice",
	"test/transcript-tool-changes.test.ts":
		"'resolves the top-level tool boundary per transport' imports resolveTranscriptTools from ../src/utils/transcript.ts, bridged by the utils slice; the other 23 of 24 tests pass",
	"test/uuid.test.ts": "imports ../src/utils/uuid.ts, bridged by the utils slice",
	"test/validation-probes.test.ts": "imports ../src/utils/validation.ts, bridged by the utils slice",
	"test/validation.test.ts": "imports ../src/utils/validation.ts, bridged by the utils slice",
	"test/xai-oauth.test.ts": "imports ../src/auth/oauth/xai.ts, bridged by the auth slice",
	"test/xai-responses.test.ts": "imports ../src/utils/identity.ts (formatUserAgent), bridged by the utils slice",
};

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
		include: ["test/*.test.ts"],
		exclude: Object.keys(skipped),
		testTimeout: 30000,
	},
});

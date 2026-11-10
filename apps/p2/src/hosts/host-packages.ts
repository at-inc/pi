// The host packages of p2's facets: the modules a facet shares with its host as instances, by specifier. Apart from
// plugins.ts (the server's plugin component, which builds with esbuild) so the worker entry imports only these: a
// worker loads neither esbuild nor the server's API.
import * as piAi from "@at-inc/pi-ai";
import * as anthropicMessages from "@at-inc/pi-ai/api/anthropic-messages.lazy";
import * as googleGenerativeAi from "@at-inc/pi-ai/api/google-generative-ai.lazy";
import * as openaiCompletions from "@at-inc/pi-ai/api/openai-completions.lazy";
import * as openaiResponses from "@at-inc/pi-ai/api/openai-responses.lazy";
import * as piAiModels from "@at-inc/pi-ai/models";
import * as allProviders from "@at-inc/pi-ai/providers/all";
import * as fauxProvider from "@at-inc/pi-ai/providers/faux";
import * as radiusProvider from "@at-inc/pi-ai/providers/radius";
import * as radiusConfig from "@at-inc/pi-ai/providers/radius-config";
import * as piCodemode from "@earendil-works/pi-codemode";
import * as piDurable from "@earendil-works/pi-durable";
import * as durableCodemode from "@earendil-works/pi-durable/codemode";
import * as durableEnv from "@earendil-works/pi-durable/env";
import * as durableEnvNode from "@earendil-works/pi-durable/env/node";
import * as durableToolSearch from "@earendil-works/pi-durable/tool-search";
import * as durableTools from "@earendil-works/pi-durable/tools";
import * as piEnv from "@earendil-works/pi-env";
import * as piMcp from "@earendil-works/pi-mcp";
import * as piMcpOauth from "@earendil-works/pi-mcp/oauth";
import * as piSchema from "@earendil-works/pi-schema";
import * as schemaFormats from "@earendil-works/pi-schema/formats";
import * as piTui from "@earendil-works/pi-tui";
import * as ws from "ws";
import * as yaml from "yaml";
import * as api from "../api/v1/index.ts";
import * as apiNode from "../api/v1/node.ts";
import * as apiWorker from "../api/v1/worker.ts";

/** The packages a facet shares with its host as instances (never bundled into a plugin's script), by specifier. */
export const SHARED_PACKAGES = {
	"@at-inc/pi-ai": piAi,
	"@at-inc/pi-ai/api/anthropic-messages.lazy": anthropicMessages,
	"@at-inc/pi-ai/api/google-generative-ai.lazy": googleGenerativeAi,
	"@at-inc/pi-ai/api/openai-completions.lazy": openaiCompletions,
	"@at-inc/pi-ai/api/openai-responses.lazy": openaiResponses,
	"@at-inc/pi-ai/models": piAiModels,
	"@at-inc/pi-ai/providers/all": allProviders,
	"@at-inc/pi-ai/providers/faux": fauxProvider,
	"@at-inc/pi-ai/providers/radius": radiusProvider,
	"@at-inc/pi-ai/providers/radius-config": radiusConfig,
	"@earendil-works/pi-codemode": piCodemode,
	"@earendil-works/pi-durable": piDurable,
	"@earendil-works/pi-durable/codemode": durableCodemode,
	"@earendil-works/pi-durable/env": durableEnv,
	"@earendil-works/pi-durable/env/node": durableEnvNode,
	"@earendil-works/pi-durable/tool-search": durableToolSearch,
	"@earendil-works/pi-durable/tools": durableTools,
	"@earendil-works/pi-env": piEnv,
	"@earendil-works/pi-mcp": piMcp,
	"@earendil-works/pi-mcp/oauth": piMcpOauth,
	"@earendil-works/pi-schema": piSchema,
	"@earendil-works/pi-schema/formats": schemaFormats,
	ws,
	yaml,
};

/** The host packages of worker facets (pi-tui too: file completion ranks as pi-tui's autocomplete does). */
export const WORKER_PACKAGES = {
	"@p2/app/v1": api,
	"@p2/app/v1/node": apiNode,
	"@p2/app/v1/worker": apiWorker,
	"@earendil-works/pi-tui": piTui,
	...SHARED_PACKAGES,
};

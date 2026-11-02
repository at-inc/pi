import * as bundledPiAgentCore from "@at-inc/pi-agent-core";
import * as bundledPiAiModels from "@at-inc/pi-ai/models";
import * as bundledPiAiOauth from "@at-inc/pi-ai/oauth";
import * as bundledPiAiProviders from "@at-inc/pi-ai/providers/all";
import * as bundledPiTui from "@earendil-works/pi-tui";
import * as bundledTypebox from "typebox";
import * as bundledTypeboxCompile from "typebox/compile";
import * as bundledTypeboxValue from "typebox/value";
// This import is safe because loader.ts exports are not re-exported from index.ts.
// Extensions can therefore import from @at-inc/pi.
import * as bundledPiCodingAgent from "../../index.ts";
import * as bundledPiAiExtensionView from "./pi-ai-extension-view.ts";

/** Modules available to extensions in source and compiled binary runtimes. */
export const VIRTUAL_MODULES: Record<string, unknown> = {
	typebox: bundledTypebox,
	"typebox/compile": bundledTypeboxCompile,
	"typebox/value": bundledTypeboxValue,
	"@sinclair/typebox": bundledTypebox,
	"@sinclair/typebox/compile": bundledTypeboxCompile,
	"@sinclair/typebox/value": bundledTypeboxValue,
	"@at-inc/pi-agent-core": bundledPiAgentCore,
	"@earendil-works/pi-tui": bundledPiTui,
	// Extensions resolve the pi-ai root to the compat entrypoint (a strict
	// superset of the core entrypoint) plus TypeBox's Type, Static and TSchema:
	// existing extensions using the old global API and pi-ai's former TypeBox
	// re-exports keep working.
	"@at-inc/pi-ai": bundledPiAiExtensionView,
	"@at-inc/pi-ai/compat": bundledPiAiExtensionView,
	"@at-inc/pi-ai/models": bundledPiAiModels,
	"@at-inc/pi-ai/oauth": bundledPiAiOauth,
	"@at-inc/pi-ai/providers/all": bundledPiAiProviders,
	"@at-inc/pi": bundledPiCodingAgent,
	"@mariozechner/pi-agent-core": bundledPiAgentCore,
	"@mariozechner/pi-tui": bundledPiTui,
	"@mariozechner/pi-ai": bundledPiAiExtensionView,
	"@mariozechner/pi-ai/compat": bundledPiAiExtensionView,
	"@mariozechner/pi-ai/models": bundledPiAiModels,
	"@mariozechner/pi-ai/oauth": bundledPiAiOauth,
	"@mariozechner/pi-ai/providers/all": bundledPiAiProviders,
	"@mariozechner/pi-coding-agent": bundledPiCodingAgent,
};

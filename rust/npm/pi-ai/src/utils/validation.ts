// utils/validation: tool argument validation with the Rust pi-schema; tools and calls cross as JSON text. As the TS
// keeps one compiled validator per schema object, the facade keeps the JSON of each schema object as first seen
// (WeakMap) and validates with it, so the Rust validator cache, keyed by schema JSON, returns that validator.

import type { Tool } from "@at-inc/pi-ai";
import type * as ts from "@at-inc/pi-ai/utils/validation";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/validation";

const firstSeenSchemas = new WeakMap<object, Tool["parameters"]>();

function withFirstSeenSchema(tool: Tool): Tool {
	let parameters = firstSeenSchemas.get(tool.parameters);
	if (!parameters) {
		parameters = JSON.parse(JSON.stringify(tool.parameters)) as Tool["parameters"];
		firstSeenSchemas.set(tool.parameters, parameters);
	}
	return { ...tool, parameters };
}

export const validateToolCall: typeof ts.validateToolCall = (tools, toolCall) =>
	wasm.aiValidateToolCall(tools.map(withFirstSeenSchema), toolCall);

export const validateToolArguments: typeof ts.validateToolArguments = (tool, toolCall) =>
	wasm.aiValidateToolArguments(withFirstSeenSchema(tool), toolCall);

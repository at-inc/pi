// utils/json-parse: JSON repair and tolerant parsing in the Rust core (a port of V8's JSON.parse and partial-json).

import type * as ts from "@at-inc/pi-ai/utils/json-parse";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/json-parse";

export const repairJson: typeof ts.repairJson = (json) => wasm.aiRepairJson(json);

export const parseJsonWithRepair = ((json: string) =>
	wasm.aiParseJsonWithRepair(json)) as typeof ts.parseJsonWithRepair;

export const parseToolCallArguments: typeof ts.parseToolCallArguments = (json) => wasm.aiParseToolCallArguments(json);

export const parseStreamingJson = ((partialJson: string | undefined) =>
	wasm.aiParseStreamingJson(partialJson)) as typeof ts.parseStreamingJson;

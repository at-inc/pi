// api/bedrock-converse-stream.lazy: the Amazon Bedrock Converse Stream API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/bedrock-converse-stream.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi, nativeOf } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/bedrock-converse-stream.lazy";

export const setBedrockProviderModule: typeof ts.setBedrockProviderModule = (module) =>
	wasm.aiSetBedrockProviderModule(module, nativeOf(module)?.share());

export const bedrockConverseStreamApi: typeof ts.bedrockConverseStreamApi = () =>
	nativeApi(wasm.aiBedrockConverseStreamApi());

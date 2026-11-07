// bedrock-provider: the Bedrock implementation as a module object for setBedrockProviderModule. It carries its Rust
// implementation, so registering it keeps the calls in Rust.

import type * as ts from "@at-inc/pi-ai/bedrock-provider";
import { wasm } from "@at-inc/pi-core";
import { stream, streamSimple } from "./api/bedrock-converse-stream.ts";
import { nativeApi } from "./internal/native-api.ts";

export const bedrockProviderModule: typeof ts.bedrockProviderModule = {
	...nativeApi(wasm.aiBedrockConverseStreamStreams()),
	stream,
	streamSimple,
};

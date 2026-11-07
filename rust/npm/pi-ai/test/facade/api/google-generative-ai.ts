import * as api from "../../../src/api/google-generative-ai.ts";
import { withSdkChunks } from "../google-bridge.ts";

export * from "../../../src/api/google-generative-ai.ts";

export const stream = withSdkChunks(api.stream);
export const streamSimple = withSdkChunks(api.streamSimple);

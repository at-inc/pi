import * as api from "../../../src/api/google-vertex.ts";
import { withSdkChunks } from "../google-bridge.ts";

export * from "../../../src/api/google-vertex.ts";

export const stream = withSdkChunks(api.stream);
export const streamSimple = withSdkChunks(api.streamSimple);

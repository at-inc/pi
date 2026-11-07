// api/google-vertex.lazy: the Google Vertex AI API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/google-vertex.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/google-vertex.lazy";

export const googleVertexApi: typeof ts.googleVertexApi = () => nativeApi(wasm.aiGoogleVertexApi());

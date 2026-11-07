// api/cloudflare-workers-ai-system-one.lazy: the API as ProviderClassifier.

import type * as ts from "@at-inc/pi-ai/api/cloudflare-workers-ai-system-one.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/cloudflare-workers-ai-system-one.lazy";

export const cloudflareWorkersAISystemOneApi: typeof ts.cloudflareWorkersAISystemOneApi = () =>
	nativeClassifier(wasm.aiCloudflareWorkersAISystemOneApi());

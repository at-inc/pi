// api/cloudflare-workers-ai-system-one: `classify` over the Rust API module.

import type * as ts from "@at-inc/pi-ai/api/cloudflare-workers-ai-system-one";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/cloudflare-workers-ai-system-one";

const api = nativeClassifier(wasm.aiCloudflareWorkersAISystemOneApi());

export const classify: typeof ts.classify = (model, context, options) => api.classify(model, context, options);

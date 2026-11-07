// api/openai-decisions: `classify` over the Rust API module.

import type * as ts from "@at-inc/pi-ai/api/openai-decisions";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-decisions";

const api = nativeClassifier(wasm.aiOpenAIDecisionsApi());

export const classify: typeof ts.classify = (model, context, options) => api.classify(model, context, options);

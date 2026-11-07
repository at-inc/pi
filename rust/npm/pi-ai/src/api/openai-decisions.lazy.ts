// api/openai-decisions.lazy: the API as ProviderClassifier.

import type * as ts from "@at-inc/pi-ai/api/openai-decisions.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/openai-decisions.lazy";

export const openAIDecisionsApi: typeof ts.openAIDecisionsApi = () => nativeClassifier(wasm.aiOpenAIDecisionsApi());

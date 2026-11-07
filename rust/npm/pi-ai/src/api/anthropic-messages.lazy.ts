// api/anthropic-messages.lazy: the Anthropic Messages API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/anthropic-messages.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/anthropic-messages.lazy";

export const anthropicMessagesApi: typeof ts.anthropicMessagesApi = () => nativeApi(wasm.aiAnthropicMessagesApi());

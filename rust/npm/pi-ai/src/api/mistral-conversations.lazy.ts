// api/mistral-conversations.lazy: the Mistral Conversations API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/mistral-conversations.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/mistral-conversations.lazy";

export const mistralConversationsApi: typeof ts.mistralConversationsApi = () =>
	nativeApi(wasm.aiMistralConversationsApi());

// api/pi-messages.lazy: the pi messages API as ProviderStreams.

import type * as ts from "@at-inc/pi-ai/api/pi-messages.lazy";
import { wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/pi-messages.lazy";

export const piMessagesApi: typeof ts.piMessagesApi = () => nativeApi(wasm.aiPiMessagesApi());

// api/typesafe-system-one: `classify` over the Rust API module.

import type * as ts from "@at-inc/pi-ai/api/typesafe-system-one";
import { wasm } from "@at-inc/pi-core";
import { nativeClassifier } from "../internal/native-api.ts";

export type * from "@at-inc/pi-ai/api/typesafe-system-one";

const api = nativeClassifier(wasm.aiTypesafeSystemOneApi());

export const classify: typeof ts.classify = (model, context, options) => api.classify(model, context, options);

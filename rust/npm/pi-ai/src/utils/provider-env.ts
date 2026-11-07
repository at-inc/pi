// utils/provider-env: `env`, then the host's environment, read in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/provider-env";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/provider-env";

export const getProviderEnvValue: typeof ts.getProviderEnvValue = (name, env) => wasm.aiGetProviderEnvValue(name, env);

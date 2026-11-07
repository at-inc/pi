// bun-oauth: registers the bundled OAuth flows, which the Rust core links statically.

import type * as ts from "@at-inc/pi-ai/bun-oauth";
import { wasm } from "@at-inc/pi-core";

export const registerBunOAuthFlows: typeof ts.registerBunOAuthFlows = () => wasm.aiRegisterBunOAuthFlows();

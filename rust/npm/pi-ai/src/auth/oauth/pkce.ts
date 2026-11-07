// auth/oauth/pkce: PKCE verifier and challenge.

import type * as ts from "@at-inc/pi-ai/auth/oauth/pkce";
import { wasm } from "@at-inc/pi-core";

export const generatePKCE: typeof ts.generatePKCE = () => wasm.aiGeneratePKCE();

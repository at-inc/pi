// auth/oauth/trusted-url: the http(s) URLs login flows may open.

import type * as ts from "@at-inc/pi-ai/auth/oauth/trusted-url";
import { wasm } from "@at-inc/pi-core";

export const trustedHttpUrl: typeof ts.trustedHttpUrl = (value) => wasm.aiTrustedHttpUrl(value);

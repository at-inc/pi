// auth/context: the default auth context over the host's environment and file system.

import type * as ts from "@at-inc/pi-ai";
import { wasm } from "@at-inc/pi-core";
import { authContextOf } from "../internal/auth.ts";

export const defaultProviderAuthContext: typeof ts.defaultProviderAuthContext = () =>
	authContextOf(wasm.aiDefaultProviderAuthContext());

// utils/node-http-proxy: proxy resolution in the Rust core, from `env` and then the host's environment.

import type * as ts from "@at-inc/pi-ai/utils/node-http-proxy";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/node-http-proxy";

export const UNSUPPORTED_PROXY_PROTOCOL_MESSAGE: typeof ts.UNSUPPORTED_PROXY_PROTOCOL_MESSAGE =
	wasm.aiUnsupportedProxyProtocolMessage() as typeof ts.UNSUPPORTED_PROXY_PROTOCOL_MESSAGE;

export const resolveHttpProxyUrlForTarget: typeof ts.resolveHttpProxyUrlForTarget = (targetUrl, env) =>
	wasm.aiResolveHttpProxyUrlForTarget(targetUrl, env);

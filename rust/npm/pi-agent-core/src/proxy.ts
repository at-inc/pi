// The facade of proxy.ts: `streamProxy` over the Rust proxy stream (see rust/crates/pi-wasm/src/agent/proxy.rs).

import type * as ts from "@at-inc/pi-agent-core";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type { ProxyAssistantMessageEvent, ProxyStreamOptions } from "@at-inc/pi-agent-core";

/**
 * Stream function that proxies through a server instead of calling LLM providers directly.
 * The server strips the partial field from delta events to reduce bandwidth.
 * We reconstruct the partial message client-side.
 *
 * Use this as the `streamFn` option when creating an Agent that needs to go through a proxy.
 */
export const streamProxy: typeof ts.streamProxy = (model, context, options) =>
	streamFrom((sink) => wasm.agentStreamProxy(model, context, options, sink)) as unknown as ReturnType<
		typeof ts.streamProxy
	>;

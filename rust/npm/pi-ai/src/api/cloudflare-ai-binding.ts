// api/cloudflare-ai-binding: a `fetch` over the Workers AI binding. The Rust module's `createAiBindingFetch` only checks
// that the binding has `fetch` and hands that function back, so the facade does the same check on the JS binding and
// returns its bound `fetch`, which the core calls like any caller-supplied fetch.

import type * as ts from "@at-inc/pi-ai/api/cloudflare-ai-binding";

export type * from "@at-inc/pi-ai/api/cloudflare-ai-binding";

export const CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL: typeof ts.CLOUDFLARE_GATEWAY_BINDING_AUTH_SENTINEL =
	"cloudflare-gateway-binding";

export const createAiBindingFetch: typeof ts.createAiBindingFetch = (binding) => {
	if (typeof binding.fetch !== "function") {
		throw new TypeError("createAiBindingFetch: the AI binding does not expose fetch()");
	}
	const bindingFetch = binding.fetch.bind(binding);
	return (input, init) => bindingFetch(input, init);
};

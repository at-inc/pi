// api/cloudflare: the Cloudflare endpoint templates, the values of the Rust module's constants.

import type * as ts from "@at-inc/pi-ai/api/cloudflare";

export const CLOUDFLARE_WORKERS_AI_BASE_URL: typeof ts.CLOUDFLARE_WORKERS_AI_BASE_URL =
	"https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai/v1";

export const CLOUDFLARE_WORKERS_AI_REST_BASE_URL: typeof ts.CLOUDFLARE_WORKERS_AI_REST_BASE_URL =
	"https://api.cloudflare.com/client/v4/accounts/{CLOUDFLARE_ACCOUNT_ID}/ai";

export const CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL: typeof ts.CLOUDFLARE_AI_GATEWAY_COMPAT_BASE_URL =
	"https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/compat";

export const CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL: typeof ts.CLOUDFLARE_AI_GATEWAY_OPENAI_BASE_URL =
	"https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/openai";

export const CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL: typeof ts.CLOUDFLARE_AI_GATEWAY_ANTHROPIC_BASE_URL =
	"https://gateway.ai.cloudflare.com/v1/{CLOUDFLARE_ACCOUNT_ID}/{CLOUDFLARE_GATEWAY_ID}/anthropic";

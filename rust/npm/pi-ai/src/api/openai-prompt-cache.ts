// api/openai-prompt-cache: the length limit of OpenAI prompt cache keys.

import type * as ts from "@at-inc/pi-ai/api/openai-prompt-cache";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/openai-prompt-cache";

export const OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH =
	wasm.aiOpenAIPromptCacheKeyMaxLength() as typeof ts.OPENAI_PROMPT_CACHE_KEY_MAX_LENGTH;

export const clampOpenAIPromptCacheKey: typeof ts.clampOpenAIPromptCacheKey = (key) =>
	wasm.aiClampOpenAIPromptCacheKey(key);

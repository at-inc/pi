// auth/oauth/openai-chatgpt: the OpenAI (Sign in with ChatGPT) OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/openai-chatgpt";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const openaiChatGPTOAuth: typeof ts.openaiChatGPTOAuth = oauthAuthOf(wasm.aiOpenaiChatGPTOAuth());

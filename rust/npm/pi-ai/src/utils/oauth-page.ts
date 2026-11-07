// utils/oauth-page: the OAuth result pages rendered in the Rust core.

import type * as ts from "@at-inc/pi-ai/utils/oauth-page";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/oauth-page";

export const oauthSuccessHtml: typeof ts.oauthSuccessHtml = (message) => wasm.aiOauthSuccessHtml(message);

export const oauthErrorHtml: typeof ts.oauthErrorHtml = (message, details) => wasm.aiOauthErrorHtml(message, details);

// auth/oauth/github-copilot: the GitHub Copilot OAuth flow of the Rust core.

import type * as ts from "@at-inc/pi-ai/auth/oauth/github-copilot";
import { wasm } from "@at-inc/pi-core";
import { oauthAuthOf } from "../../internal/auth.ts";

export const githubCopilotOAuth: typeof ts.githubCopilotOAuth = oauthAuthOf(wasm.aiGithubCopilotOAuth());

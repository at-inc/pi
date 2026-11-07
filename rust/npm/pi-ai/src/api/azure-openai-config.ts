// api/azure-openai-config: the Azure OpenAI endpoint (base URL, API version, deployment) of a request.

import type * as ts from "@at-inc/pi-ai/api/azure-openai-config";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/azure-openai-config";

export const resolveDeploymentName: typeof ts.resolveDeploymentName = (model, options) =>
	wasm.aiResolveDeploymentName(model, options);

export const resolveAzureBaseUrl: typeof ts.resolveAzureBaseUrl = (model, options) =>
	wasm.aiResolveAzureBaseUrl(model, options);

export const resolveAzureConfig: typeof ts.resolveAzureConfig = (model, options) =>
	wasm.aiResolveAzureConfig(model, options);

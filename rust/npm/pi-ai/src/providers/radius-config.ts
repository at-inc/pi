// providers/radius-config: the Radius gateway configuration helpers, over the Rust core.

import type * as ts from "@at-inc/pi-ai/providers/radius-config";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/providers/radius-config";

export const DEFAULT_RADIUS_GATEWAY: typeof ts.DEFAULT_RADIUS_GATEWAY =
	wasm.aiDefaultRadiusGateway() as typeof ts.DEFAULT_RADIUS_GATEWAY;

export const normalizeRadiusGatewayUrl: typeof ts.normalizeRadiusGatewayUrl = (value) =>
	wasm.aiNormalizeRadiusGatewayUrl(value);

export const getRadiusCredentialConfig: typeof ts.getRadiusCredentialConfig = (credential) =>
	wasm.aiGetRadiusCredentialConfig(credential);

export const getRadiusModelsFromConfig: typeof ts.getRadiusModelsFromConfig = (providerId, config) =>
	wasm.aiGetRadiusModelsFromConfig(providerId, config);

export const getRadiusModels: typeof ts.getRadiusModels = (providerId, credential) =>
	wasm.aiGetRadiusModels(providerId, credential);

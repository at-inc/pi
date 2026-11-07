// providers/cloudflare-stream: the Cloudflare endpoint resolution and the stream and classifier wrappers, over the
// Rust provider module.

import type * as ts from "@at-inc/pi-ai/providers/cloudflare-stream";
import { wasm } from "@at-inc/pi-core";
import { nativeApi, nativeClassifier, nativeClassifierOf, nativeOf } from "../internal/native-api.ts";

export const resolveCloudflareModel: typeof ts.resolveCloudflareModel = (model, env) => {
	if (!env) return model;
	const baseUrl = wasm.aiResolveCloudflareBaseUrl(model, env);
	return baseUrl === model.baseUrl ? model : { ...model, baseUrl };
};

export const cloudflareStreams: typeof ts.cloudflareStreams = (streams) =>
	nativeApi(wasm.aiCloudflareStreams(nativeOf(streams)?.share(), streams));

export const cloudflareClassifier: typeof ts.cloudflareClassifier = (classifier) =>
	nativeClassifier(wasm.aiCloudflareClassifier(nativeClassifierOf(classifier)?.share(), classifier));

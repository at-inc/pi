// The JS values of Rust API implementations: ProviderStreams, ProviderImages and ProviderClassifier objects that keep
// their Rust handle, so createProvider passes the Rust implementation back to Rust without a JS hop.

import type { ProviderClassifier, ProviderImages, ProviderStreams } from "@at-inc/pi-ai";
import { streamFrom } from "@at-inc/pi-core";
import type { NativeClassifier, NativeImages, NativeStreams } from "@at-inc/pi-core/wasm";
import { rethrow } from "./errors.ts";

const NATIVE = Symbol("pi-core native implementation");

/** A ProviderStreams that also carries its Rust implementation. */
export interface NativeProviderStreams extends ProviderStreams {
	readonly [NATIVE]: NativeStreams;
}

/** Wraps a Rust API implementation as ProviderStreams, with the deferred methods the implementation has. */
export function nativeApi(native: NativeStreams): NativeProviderStreams {
	const api: NativeProviderStreams = {
		stream: (model, context, options) => streamFrom((sink) => native.stream(model, context, options, sink)),
		streamSimple: (model, context, options) =>
			streamFrom((sink) => native.streamSimple(model, context, options, sink)),
		[NATIVE]: native,
	};
	if (native.hasFetchDeferred()) {
		api.fetchDeferred = (model, handle, options) =>
			streamFrom((sink) => native.fetchDeferred(model, handle, options, sink));
	}
	if (native.hasCancelDeferred()) {
		api.cancelDeferred = (model, handle, options) => native.cancelDeferred(model, handle, options).catch(rethrow);
	}
	return api;
}

/** The Rust implementation of a ProviderStreams value, when it is one from nativeApi. */
export function nativeOf(streams: ProviderStreams | undefined): NativeStreams | undefined {
	return (streams as Partial<NativeProviderStreams> | undefined)?.[NATIVE];
}

/** Wraps a Rust image-generation implementation as ProviderImages. */
export function nativeImages(native: NativeImages): ProviderImages {
	return {
		generateImages: (model, context, options) => native.generateImages(model, context, options).catch(rethrow),
		[NATIVE]: native,
	} as ProviderImages;
}

/** The Rust implementation of a ProviderImages value, when it is one from nativeImages. */
export function nativeImagesOf(images: ProviderImages | undefined): NativeImages | undefined {
	return (images as { [NATIVE]?: NativeImages } | undefined)?.[NATIVE];
}

/** Wraps a Rust classifier implementation as ProviderClassifier. */
export function nativeClassifier(native: NativeClassifier): ProviderClassifier {
	return {
		classify: (model, context, options) => native.classify(model, context, options).catch(rethrow),
		[NATIVE]: native,
	} as ProviderClassifier;
}

/** The Rust implementation of a ProviderClassifier value, when it is one from nativeClassifier. */
export function nativeClassifierOf(classifier: ProviderClassifier | undefined): NativeClassifier | undefined {
	return (classifier as { [NATIVE]?: NativeClassifier } | undefined)?.[NATIVE];
}

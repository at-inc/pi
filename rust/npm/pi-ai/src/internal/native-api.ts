// The JS ProviderStreams of a Rust API implementation. It keeps the NativeStreams handle so createProvider passes the
// Rust implementation back to Rust without a JS hop.

import type { ProviderStreams } from "@at-inc/pi-ai";
import { streamFrom } from "@at-inc/pi-core";
import type { NativeStreams } from "@at-inc/pi-core/wasm";

const NATIVE = Symbol("pi-core NativeStreams");

/** A ProviderStreams that also carries its Rust implementation. */
export interface NativeProviderStreams extends ProviderStreams {
	readonly [NATIVE]: NativeStreams;
}

/** Wraps a Rust API implementation as ProviderStreams. */
export function nativeApi(native: NativeStreams): NativeProviderStreams {
	return {
		stream: (model, context, options) => streamFrom((sink) => native.stream(model, context, options, sink)),
		streamSimple: (model, context, options) =>
			streamFrom((sink) => native.streamSimple(model, context, options, sink)),
		[NATIVE]: native,
	};
}

/** The Rust implementation of a ProviderStreams value, when it is one from nativeApi. */
export function nativeOf(streams: ProviderStreams | undefined): NativeStreams | undefined {
	return (streams as Partial<NativeProviderStreams> | undefined)?.[NATIVE];
}

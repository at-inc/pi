// api/lazy: `lazyStream` and `lazyApi`, which run JS setup and module loading behind a stream in the Rust core.

import type * as ts from "@at-inc/pi-ai";
import { streamFrom, wasm } from "@at-inc/pi-core";
import { nativeApi } from "../internal/native-api.ts";

export type { LazyApiCapabilities, LazyStreamObserver } from "@at-inc/pi-ai";

export const lazyStream: typeof ts.lazyStream = (model, setup, observer) =>
	streamFrom((sink) => wasm.aiLazyStream(model, setup, observer, sink));

export const lazyApi: typeof ts.lazyApi = (load, capabilities) =>
	nativeApi(wasm.aiLazyApi(load, capabilities?.fetchDeferred, capabilities?.cancelDeferred));

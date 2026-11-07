// models-store: the in-memory models store, a Rust store behind the TS class's methods.

import type * as ts from "@at-inc/pi-ai/models-store";
import { wasm } from "@at-inc/pi-core";
import type { ModelsStoreHandle } from "@at-inc/pi-core/wasm";

export type * from "@at-inc/pi-ai/models-store";

/** The TS `InMemoryModelsStore` over a Rust store. */
class RustInMemoryModelsStore implements ts.ModelsStore {
	readonly #handle: ModelsStoreHandle = wasm.aiInMemoryModelsStore();

	read(...[providerId, options]: Parameters<ts.ModelsStore["read"]>): ReturnType<ts.ModelsStore["read"]> {
		return this.#handle.read(providerId, options);
	}

	write(...[providerId, entry, options]: Parameters<ts.ModelsStore["write"]>): ReturnType<ts.ModelsStore["write"]> {
		return this.#handle.write(providerId, entry, options);
	}

	delete(...[providerId, options]: Parameters<ts.ModelsStore["delete"]>): ReturnType<ts.ModelsStore["delete"]> {
		return this.#handle.delete(providerId, options);
	}
}

// The TS class has private members, which makes TypeScript compare it by declaration.
export const InMemoryModelsStore = RustInMemoryModelsStore as unknown as typeof ts.InMemoryModelsStore;
export type InMemoryModelsStore = ts.InMemoryModelsStore;

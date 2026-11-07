// models-store: `InMemoryModelsStore` over the Rust store. A `Models` given one calls it like any JS store.

import type { ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions } from "@at-inc/pi-ai";
import { wasm } from "@at-inc/pi-core";
import { rethrow } from "./internal/errors.ts";

export type { ModelsStore, ModelsStoreEntry, ModelsStoreOperationOptions } from "@at-inc/pi-ai";

/** The TS package's `InMemoryModelsStore`: entries are copied in and out, as `structuredClone` does in the TS. */
export class InMemoryModelsStore implements ModelsStore {
	readonly #handle = wasm.aiInMemoryModelsStore();

	read(providerId: string, options?: ModelsStoreOperationOptions): Promise<ModelsStoreEntry | undefined> {
		return this.#handle.read(providerId, options).catch(rethrow);
	}

	async write(providerId: string, entry: ModelsStoreEntry, options?: ModelsStoreOperationOptions): Promise<void> {
		return this.#handle.write(providerId, entry, options).catch(rethrow);
	}

	delete(providerId: string, options?: ModelsStoreOperationOptions): Promise<void> {
		return this.#handle.delete(providerId, options).catch(rethrow);
	}
}

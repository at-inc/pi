// auth/credential-store: the in-memory credential store, a Rust store behind the TS class's methods.

import type * as ts from "@at-inc/pi-ai";
import { wasm } from "@at-inc/pi-core";
import type { CredentialStoreHandle } from "@at-inc/pi-core/wasm";

/** The TS `InMemoryCredentialStore` over a Rust store. Writes are serialized per provider in the core. */
class RustInMemoryCredentialStore implements ts.CredentialStore {
	readonly #handle: CredentialStoreHandle = wasm.aiInMemoryCredentialStore();

	read(...[providerId, options]: Parameters<ts.CredentialStore["read"]>): ReturnType<ts.CredentialStore["read"]> {
		return this.#handle.read(providerId, options);
	}

	list(...[options]: Parameters<ts.CredentialStore["list"]>): ReturnType<ts.CredentialStore["list"]> {
		return this.#handle.list(options);
	}

	modify(
		...[providerId, fn, options]: Parameters<ts.CredentialStore["modify"]>
	): ReturnType<ts.CredentialStore["modify"]> {
		return this.#handle.modify(providerId, fn, options);
	}

	delete(
		...[providerId, options]: Parameters<ts.CredentialStore["delete"]>
	): ReturnType<ts.CredentialStore["delete"]> {
		return this.#handle.delete(providerId, options);
	}
}

// The TS class has private members, which makes TypeScript compare it by declaration.
export const InMemoryCredentialStore = RustInMemoryCredentialStore as unknown as typeof ts.InMemoryCredentialStore;
export type InMemoryCredentialStore = ts.InMemoryCredentialStore;

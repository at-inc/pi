// Errors from the core: a Rust ModelsError crosses as a JS Error named "ModelsError" with its `code`; reviving gives it
// the facade's ModelsError prototype, so `instanceof ModelsError` holds as in the TS.

import { ModelsError } from "../utils/models-error.ts";

/** `error`, as a ModelsError instance when the core made one. */
export function revived(error: unknown): unknown {
	if (error instanceof Error && error.name === "ModelsError" && !(error instanceof ModelsError)) {
		Object.setPrototypeOf(error, ModelsError.prototype);
	}
	return error;
}

/** Throws `revived(error)`, for a rejected core promise. */
export function rethrow(error: unknown): never {
	throw revived(error);
}

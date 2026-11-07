// utils/models-error: the error class of Models operations. Its message carries the cause's detail, computed by the core.

import type * as ts from "@at-inc/pi-ai/models";
import { wasm } from "@at-inc/pi-core";

export type { ModelsErrorCode } from "@at-inc/pi-ai/models";

/** The TS package's `ModelsError`; errors the core throws with this name are revived to it (`internal/errors.ts`). */
export class ModelsError extends Error {
	readonly code: ts.ModelsErrorCode;

	constructor(code: ts.ModelsErrorCode, message: string, options?: { cause?: unknown }) {
		super(wasm.aiModelsErrorMessage(message, options?.cause), options);
		this.name = "ModelsError";
		this.code = code;
	}
}

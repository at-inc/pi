// utils/diagnostics: diagnostics in the Rust core. Errors keep their JS `stack`.

import type * as ts from "@at-inc/pi-ai/utils/diagnostics";
import { wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/utils/diagnostics";

export const formatThrownValue: typeof ts.formatThrownValue = (value) => wasm.aiFormatThrownValue(value);

export const extractDiagnosticError: typeof ts.extractDiagnosticError = (error) => wasm.aiExtractDiagnosticError(error);

export const createAssistantMessageDiagnostic: typeof ts.createAssistantMessageDiagnostic = (type, error, details) =>
	wasm.aiCreateAssistantMessageDiagnostic(type, error, details);

export const createProviderErrorDiagnostic: typeof ts.createProviderErrorDiagnostic = (error) =>
	wasm.aiCreateProviderErrorDiagnostic(error);

export const appendAssistantMessageDiagnostic: typeof ts.appendAssistantMessageDiagnostic = (message, diagnostic) =>
	wasm.aiAppendAssistantMessageDiagnostic(message, diagnostic);

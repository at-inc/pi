export interface DiagnosticErrorInfo {
	name?: string;
	message: string;
	stack?: string;
	code?: string | number;
}

export interface AssistantMessageDiagnostic {
	type: string;
	timestamp: number;
	error?: DiagnosticErrorInfo;
	details?: Record<string, unknown>;
}

export function formatThrownValue(value: unknown): string {
	if (value instanceof Error) return value.message || value.name;
	if (typeof value === "string") return value;
	return String(value);
}

export function extractDiagnosticError(error: unknown): DiagnosticErrorInfo {
	if (!(error instanceof Error)) return { name: "ThrownValue", message: formatThrownValue(error) };
	const code = (error as Error & { code?: unknown }).code;
	return {
		name: error.name || undefined,
		message: error.message || error.name,
		stack: error.stack,
		code: typeof code === "string" || typeof code === "number" ? code : undefined,
	};
}

export function createAssistantMessageDiagnostic(
	type: string,
	error: unknown,
	details?: Record<string, unknown>,
): AssistantMessageDiagnostic {
	return { type, timestamp: Date.now(), error: extractDiagnosticError(error), details };
}

/** Preserve provider metadata that is lost when SDK errors become display strings. */
export function createProviderErrorDiagnostic(error: unknown): AssistantMessageDiagnostic {
	const source = error && typeof error === "object" ? (error as Record<string, unknown>) : {};
	const status =
		typeof source.status === "number"
			? source.status
			: typeof source.statusCode === "number"
				? source.statusCode
				: undefined;
	const headers =
		source.headers instanceof Headers
			? Object.fromEntries(source.headers)
			: source.headers && typeof source.headers === "object"
				? source.headers
				: undefined;
	const payload = source.payload ?? (source.error && typeof source.error === "object" ? source.error : undefined);
	return createAssistantMessageDiagnostic("provider_error", error, { status, headers, payload });
}

export function appendAssistantMessageDiagnostic<T extends { diagnostics?: AssistantMessageDiagnostic[] }>(
	message: T,
	diagnostic: AssistantMessageDiagnostic,
): void {
	message.diagnostics = [...(message.diagnostics ?? []), diagnostic];
}

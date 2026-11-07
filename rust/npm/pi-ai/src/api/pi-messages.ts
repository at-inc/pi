// api/pi-messages: the pi messages API implementation.

import type * as ts from "@at-inc/pi-ai/api/pi-messages";
import { streamFrom, wasm } from "@at-inc/pi-core";

export type * from "@at-inc/pi-ai/api/pi-messages";

const streams = wasm.aiPiMessagesStreams();

/** An error response of a pi-messages backend, with its error code and diagnostic details. The stream reports such a
 * response as an error message, so the class has no Rust counterpart to bridge; it stays TS for callers that build or
 * test for one. */
export class PiMessagesResponseError extends Error implements ts.PiMessagesResponseError {
	code?: string;
	readonly diagnosticDetails: ts.PiMessagesResponseError["diagnosticDetails"];

	constructor(
		message: string,
		code: string | undefined,
		diagnosticDetails: ts.PiMessagesResponseError["diagnosticDetails"],
	) {
		super(message);
		this.name = "PiMessagesResponseError";
		this.code = code;
		this.diagnosticDetails = diagnosticDetails;
	}
}

export const stream: typeof ts.stream = (model, context, options) =>
	streamFrom((sink) => streams.stream(model, context, options, sink));

export const streamSimple: typeof ts.streamSimple = (model, context, options) =>
	streamFrom((sink) => streams.streamSimple(model, context, options, sink));

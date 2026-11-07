// utils/streaming-json: the parser and the tool-call arguments stream as Rust handles.

import type * as ts from "@at-inc/pi-ai/utils/streaming-json";
import { wasm } from "@at-inc/pi-core";
import type { AiStreamingJsonParser, AiToolCallArgumentsStream } from "@at-inc/pi-core/wasm";

export type * from "@at-inc/pi-ai/utils/streaming-json";

class RustStreamingJsonParser {
	readonly #handle: AiStreamingJsonParser = new wasm.AiStreamingJsonParser();

	append(text: string): readonly ts.JsonChange[] {
		return this.#handle.append(text);
	}

	snapshot(): ReturnType<ts.StreamingJsonParser["snapshot"]> {
		return this.#handle.snapshot();
	}
}

class RustToolCallArgumentsStream {
	readonly #handle: AiToolCallArgumentsStream = new wasm.AiToolCallArgumentsStream();

	get text(): string {
		return this.#handle.text;
	}

	append(delta: string): ReturnType<ts.ToolCallArgumentsStream["append"]> {
		return this.#handle.append(delta);
	}

	replace(text: string): ReturnType<ts.ToolCallArgumentsStream["replace"]> {
		return this.#handle.replace(text);
	}

	finish(): ReturnType<ts.ToolCallArgumentsStream["finish"]> {
		return this.#handle.finish();
	}
}

export const StreamingJsonParser = RustStreamingJsonParser as unknown as typeof ts.StreamingJsonParser;
export type StreamingJsonParser = ts.StreamingJsonParser;

export const ToolCallArgumentsStream = RustToolCallArgumentsStream as unknown as typeof ts.ToolCallArgumentsStream;
export type ToolCallArgumentsStream = ts.ToolCallArgumentsStream;

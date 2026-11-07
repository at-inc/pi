// Deep nesting through the facade's StreamingJsonParser. The TS test "reads deep nesting without recursion and in
// linear time" is skipped by name for its time bound (see vitest.config.ts); this keeps its correctness part: a
// 200,000-level document crosses the wasm boundary and is dropped without recursion.

import { describe, expect, it } from "vitest";
import { StreamingJsonParser } from "../../src/utils/streaming-json.ts";

type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

describe("StreamingJsonParser deep nesting", () => {
	it("reads 200,000 levels in pieces and shows the innermost value", () => {
		const depth = 200_000;
		const text = `${"[".repeat(depth)}1${"]".repeat(depth)}`;
		const parser = new StreamingJsonParser();
		for (let index = 0; index < text.length; index += 7) {
			parser.append(text.slice(index, index + 7));
			parser.snapshot();
		}
		let value = parser.snapshot() as JsonValue;
		for (let level = 0; level < depth; level++) value = (value as JsonValue[])[0]!;
		expect(value).toBe(1);
	});
});

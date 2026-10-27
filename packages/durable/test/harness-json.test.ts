import { type JsonValue, track } from "@earendil-works/chord/delta";
import { describe, expect, it } from "vitest";
import { assignJson } from "../src/harness/json.ts";

type Doc = { message?: JsonValue };

function assign(tracker: ReturnType<typeof track<Doc>>, value: JsonValue): void {
	const change = tracker.beginChange();
	assignJson(change.state as Doc, "message", value);
	tracker.adopt(change.prepare());
}

describe("assignJson", () => {
	it.each(["__proto__", "constructor", "prototype"])("preserves %s as data without traversing it", (key) => {
		const tracker = track<Doc>({});
		assign(tracker, { args: { path: "x" } });
		const args = JSON.parse(`{"path":"xy","${key}":{"polluted":true}}`);
		assign(tracker, { args });
		assign(tracker, { args: { ...args, path: "xyz" } });
		expect(({} as Record<string, unknown>).polluted).toBeUndefined();
		expect((tracker.value.message as { args: object }).args).toEqual({ ...args, path: "xyz" });
	});

	it.each(["__proto__", "constructor", "prototype"])("rejects a direct write to %s", (key) => {
		const target: Record<string, JsonValue> = {};
		expect(() => assignJson(target, key, { polluted: true })).toThrow("Reserved JSON members");
		expect(Object.getPrototypeOf(target)).toBe(Object.prototype);
		expect(({} as Record<string, unknown>).polluted).toBeUndefined();
	});

	it("does not merge into inherited objects", () => {
		const inherited = { message: { untouched: true } };
		const target: Doc = Object.create(inherited);
		assignJson(target, "message", { updated: true });
		expect(inherited.message).toEqual({ untouched: true });
		expect(Object.hasOwn(target, "message")).toBe(true);
		expect(target.message).toEqual({ updated: true });
	});

	it("does not merge a new __proto__ member into Object.prototype", () => {
		const tracker = track<Doc>({});
		assign(tracker, { args: JSON.parse('{"path":"x"}') });
		assign(tracker, { args: JSON.parse('{"path":"xy","__proto__":{"polluted":true}}') });

		expect(({} as Record<string, unknown>).polluted).toBeUndefined();
		const args = (tracker.value.message as { args: object }).args;
		expect(Object.getOwnPropertyDescriptor(args, "__proto__")?.value).toEqual({ polluted: true });
	});
});

import { expect, test } from "vitest";
import { substituteArgs } from "../src/core/prompt-templates.ts";

test("handles repeated unterminated defaults without rescanning the remaining template", () => {
	const unfinished = "${0:-|".repeat(100_000);
	expect(substituteArgs(`\${@:-${unfinished}$1`, ["value"])).toBe(`\${@:-${unfinished}value`);
});

test("keeps matching simple placeholders inside an unterminated default", () => {
	expect(substituteArgs(`\${1:-missing $2 $ARGUMENTS \${@:2}`, ["one", "two"])).toBe("one");
	expect(substituteArgs("${1:-missing $2 $ARGUMENTS", ["one", "two"])).toBe("${1:-missing two one two");
});

test("uses the first closing brace without recursively substituting the default", () => {
	expect(substituteArgs(`\${1:-nested \${2:-fallback}} $2`, [])).toBe(`nested \${2:-fallback} `);
	expect(substituteArgs(`\${1:-$2} $2`, ["", "second"])).toBe("$2 second");
});

test("advances past earlier braces when finding subsequent defaults", () => {
	expect(substituteArgs(`} \${1:-first} \${2:-second} \${3:-third} $1`, ["one"])).toBe("} one second third one");
});

test("preserves zero-length slices and very large numeric arguments", () => {
	expect(substituteArgs(`\${@:0:0}|\${@:2:0}|\${@:2:2}|$999999999999999999999`, ["a", "b", "c"])).toBe("||b c|");
});

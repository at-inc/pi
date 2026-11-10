// Generated fixture strings remain data when parsed as JavaScript or embedded in a script element.

import { Script } from "node:vm";
import { describe, expect, it } from "vitest";
import { jsString } from "./support.ts";

describe("JavaScript string literals", () => {
	it.each([
		"",
		"plain text",
		'"quoted"\\path\n\r\t\0',
		'"; globalThis.injected = true; //',
		`\`\${(() => { globalThis.injected = true; })()}\``,
		"</script><script>globalThis.injected = true;</script><!--&-->",
		"<>&\u2028\u2029<>&\u2028\u2029",
		"\ud83d\ude00\ud800\udfff",
	])("round-trips %j without executing its contents", (value) => {
		const literal = jsString(value);
		expect(literal).not.toMatch(/[<>&\u2028\u2029]/);
		expect(JSON.parse(literal)).toBe(value);
		const scope = { injected: false, value: undefined };
		new Script(`globalThis.value = ${literal};`).runInNewContext(scope);
		expect(scope.value).toBe(value);
		expect(scope.injected).toBe(false);
	});

	it("escapes every HTML delimiter and Unicode line separator after serialization", () => {
		expect(jsString("<>&\u2028\u2029")).toBe('"\\u003c\\u003e\\u0026\\u2028\\u2029"');
	});
});

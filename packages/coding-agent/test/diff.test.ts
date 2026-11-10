import { beforeAll, describe, expect, it } from "vitest";
import { renderDiff } from "../src/modes/interactive/components/diff.ts";
import { initTheme, theme } from "../src/modes/interactive/theme/theme.ts";
import { stripAnsi } from "../src/utils/ansi.ts";

describe("renderDiff", () => {
	beforeAll(() => initTheme("dark"));

	it.each([
		["+12 content", "+12 content", "toolDiffAdded"],
		["- 12 content", "- 12 content", "toolDiffRemoved"],
		[" 12 content", " 12 content", "toolDiffContext"],
		["     ...", "     ...", "toolDiffContext"],
		["\t 12x", "  12x", "toolDiffContext"],
		["+ 12x", "+ 12x", "toolDiffAdded"],
		["+12x", "+12x", "toolDiffContext"],
		["+1 x\r", "+1 x\r", "toolDiffContext"],
		["+1 x\u2028y", "+1 x\u2028y", "toolDiffContext"],
		["+1 x\u2029y", "+1 x\u2029y", "toolDiffContext"],
		["+\r12 x", "+\r12 x", "toolDiffAdded"],
		["+ \r", "+  ", "toolDiffAdded"],
		["+\tcontent\t", "+ content   ", "toolDiffAdded"],
		[" ", " ", "toolDiffContext"],
		["", "", "toolDiffContext"],
	] as const)("preserves parsing and fallback for %j", (diff, expected, color) => {
		expect(renderDiff(diff)).toBe(theme.fg(color, expected));
	});

	it("falls back on long whitespace-prefixed malformed lines", () => {
		const diff = `\t ${" ".repeat(100_000)}a\rb`;
		const started = performance.now();
		expect(renderDiff(diff)).toBe(theme.fg("toolDiffContext", diff));
		expect(performance.now() - started).toBeLessThan(2000);
	});

	it.each([
		{
			name: "text is inserted at the start of a line",
			diff: "-83   await commitRecordingChunk({\n+83   const result = await commitRecordingChunk({",
			expected: ["-83   await commitRecordingChunk({", "+83   const result = await commitRecordingChunk({"],
		},
		{
			name: "indented text is replaced",
			diff: "-12     const value = oldValue\n+12     const value = newValue",
			expected: ["-12     const value = oldValue", "+12     const value = newValue"],
		},
	])("preserves indentation when $name", ({ diff, expected }) => {
		const rendered = renderDiff(diff);

		expect(stripAnsi(rendered).split("\n")).toEqual(expected);
	});

	it("does not highlight shared indentation as an insertion", () => {
		const rendered = renderDiff(
			"-83   await commitRecordingChunk({\n+83   const result = await commitRecordingChunk({",
		);
		const inverseInsertion = theme.inverse("const result = ");

		expect(rendered.indexOf("+83   ")).toBeLessThan(rendered.indexOf(inverseInsertion));
		expect(rendered).toContain(inverseInsertion);
	});
});

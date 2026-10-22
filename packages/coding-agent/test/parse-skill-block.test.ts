import { describe, expect, it } from "vitest";
import { parseSkillBlock } from "../src/core/agent-session.ts";

describe("parseSkillBlock", () => {
	const header = '<skill name="review" location="/skills/review/SKILL.md">\n';

	it("preserves the skill body without a user message", () => {
		const content = "  # Review\n\nCheck the changes.\r\n\t";
		expect(parseSkillBlock(`${header}${content}\n</skill>`)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content,
			userMessage: undefined,
		});
	});

	it("accepts an empty body with a separate newline before the closing tag", () => {
		expect(parseSkillBlock(`${header}\n</skill>`)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content: "",
			userMessage: undefined,
		});
		expect(parseSkillBlock(`${header}</skill>`)).toBeNull();
	});

	it("trims only the user message", () => {
		expect(parseSkillBlock(`${header} body \n</skill>\n\n \tReview this.\nKeep this line.\r\n `)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content: " body ",
			userMessage: "Review this.\nKeep this line.",
		});
	});

	it.each([" ", "\n", "\r\n", "\t\u2028\u2029"])("omits a whitespace-only user message %j", (message) => {
		expect(parseSkillBlock(`${header}body\n</skill>\n\n${message}`)?.userMessage).toBeUndefined();
		expect(parseSkillBlock(`${header}body\n</skill>\n\n${message}`)?.content).toBe("body");
	});

	it("preserves non-quote characters in header values", () => {
		expect(parseSkillBlock('<skill name=" review\nname " location="/skills/\r\npath">\nbody\n</skill>')).toEqual({
			name: " review\nname ",
			location: "/skills/\r\npath",
			content: "body",
			userMessage: undefined,
		});
	});

	it.each([
		"",
		"ordinary message",
		'<skill name="" location="/skill">\nbody\n</skill>',
		'<skill name="review" location="">\nbody\n</skill>',
		'<skill location="/skill" name="review">\nbody\n</skill>',
		"<skill name='review' location='/skill'>\nbody\n</skill>",
		'<skill name="review"  location="/skill">\nbody\n</skill>',
		'<skill name="review" location="/skill">\r\nbody\n</skill>',
		`prefix${header}body\n</skill>`,
		`${header}body`,
		`${header}body</skill>`,
		`${header}body\n</SKILL>`,
	])("rejects malformed skill blocks %j", (text) => {
		expect(parseSkillBlock(text)).toBeNull();
	});

	it.each(["\n", "\r", "\r\n", "\u2028", "\u2029", "\n\n", " ", "\nmessage", "message"])(
		"rejects an invalid closing suffix %j",
		(suffix) => {
			expect(parseSkillBlock(`${header}body\n</skill>${suffix}`)).toBeNull();
		},
	);

	it("keeps closing tags with invalid suffixes in the body", () => {
		const content = "before\n</skill>not a message\n</skill>\nnot a message either";
		expect(parseSkillBlock(`${header}${content}\n</skill>\n\nrequest`)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content,
			userMessage: "request",
		});
	});

	it("uses the first closing tag with a valid suffix", () => {
		expect(parseSkillBlock(`${header}body\n</skill>\n\nmessage\n</skill>\n\nmore`)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content: "body",
			userMessage: "message\n</skill>\n\nmore",
		});
	});

	it("finds a valid closing tag after many invalid candidates", () => {
		const content = "body\n</skill>invalid".repeat(100_000);
		expect(parseSkillBlock(`${header}${content}\n</skill>`)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content,
			userMessage: undefined,
		});
	});

	it("rejects many closing tags without a valid suffix", () => {
		expect(parseSkillBlock(`${header}${"body\n</skill>invalid".repeat(100_000)}`)).toBeNull();
	});

	it("preserves many closing tags in the user message", () => {
		const message = "message\n</skill>\n\n".repeat(100_000);
		expect(parseSkillBlock(`${header}body\n</skill>\n\n${message}`)).toEqual({
			name: "review",
			location: "/skills/review/SKILL.md",
			content: "body",
			userMessage: message.trim(),
		});
	});
});

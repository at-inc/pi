import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import {
	clearConfigValueCache,
	resolveConfigValue,
	resolveConfigValueOrThrow,
	resolveConfigValueUncached,
} from "../src/core/resolve-config-value.ts";
import * as shellModule from "../src/utils/shell.ts";

describe("resolveConfigValue", () => {
	let tempDir: string;

	beforeEach(() => {
		tempDir = join(tmpdir(), `pi-config-value-${Date.now()}-${Math.random().toString(36).slice(2)}`);
		mkdirSync(tempDir, { recursive: true });
		clearConfigValueCache();
	});

	afterEach(() => {
		if (existsSync(tempDir)) rmSync(tempDir, { recursive: true });
		clearConfigValueCache();
		vi.restoreAllMocks();
	});

	test("resolves literals, environment templates, and escapes", () => {
		process.env.TEST_CONFIG_LEFT = "left";
		process.env.TEST_CONFIG_RIGHT = "right";
		try {
			expect(resolveConfigValue("literal-key")).toBe("literal-key");
			expect(resolveConfigValue("$TEST_CONFIG_LEFT")).toBe("left");
			expect(resolveConfigValue("$" + "{TEST_CONFIG_LEFT}_$TEST_CONFIG_RIGHT")).toBe("left_right");
			expect(resolveConfigValue("$$TEST_CONFIG_LEFT")).toBe("$TEST_CONFIG_LEFT");
			expect(resolveConfigValue("$!literal-$TEST_CONFIG_RIGHT")).toBe("!literal-right");
		} finally {
			delete process.env.TEST_CONFIG_LEFT;
			delete process.env.TEST_CONFIG_RIGHT;
		}
	});

	test("uses credential-scoped environment before process.env", () => {
		process.env.TEST_CONFIG_SCOPED = "process";
		try {
			expect(resolveConfigValue("$TEST_CONFIG_SCOPED", { TEST_CONFIG_SCOPED: "credential" })).toBe("credential");
		} finally {
			delete process.env.TEST_CONFIG_SCOPED;
		}
	});

	test("executes shell commands and trims their output", () => {
		expect(resolveConfigValue("!echo '  spaced-key  '")).toBe("spaced-key");
		expect(resolveConfigValue("!printf 'line1\\nline2'")).toBe("line1\nline2");
		expect(resolveConfigValue("!echo 'hello world' | tr ' ' '-'")).toBe("hello-world");
	});

	test.each(["!exit 1", "!nonexistent-command-12345", "!printf ''"])(
		"returns undefined when command resolution fails: %s",
		(command) => {
			expect(resolveConfigValue(command)).toBeUndefined();
		},
	);

	test("does not include failed credential commands or their output in errors", () => {
		vi.stubEnv("PI_TEST_CONFIG_COMMAND_OUTPUT", "CODEQL_FAKE_STDOUT_SECRET");
		vi.stubEnv("PI_TEST_CONFIG_COMMAND_ERROR", "CODEQL_FAKE_STDERR_SECRET");
		const command =
			'!echo "$PI_TEST_CONFIG_COMMAND_OUTPUT"; echo "$PI_TEST_CONFIG_COMMAND_ERROR" >&2; false CODEQL_FAKE_INLINE_SECRET';
		expect(() => resolveConfigValueOrThrow(command, "OAuth client secret")).toThrow(
			new Error("Failed to resolve OAuth client secret from shell command"),
		);
	});

	test("identifies missing environment variables without including literal credential parts", () => {
		vi.stubEnv("PI_TEST_CONFIG_MISSING", undefined);
		expect(() =>
			resolveConfigValueOrThrow("CODEQL_FAKE_LITERAL_SECRET-$PI_TEST_CONFIG_MISSING", "OAuth client secret"),
		).toThrow(new Error("Failed to resolve OAuth client secret from environment variable: PI_TEST_CONFIG_MISSING"));
	});

	test("caches successful and failed commands until explicitly cleared", () => {
		const counterFile = join(tempDir, "counter");
		writeFileSync(counterFile, "0");
		vi.stubEnv(
			"PI_TEST_COUNTER_FILE",
			process.platform === "win32" ? counterFile.replaceAll("\\", "/") : counterFile,
		);
		const success =
			'!sh -c \'count=$(cat "$PI_TEST_COUNTER_FILE"); echo $((count + 1)) > "$PI_TEST_COUNTER_FILE"; echo value\'';

		expect(resolveConfigValue(success)).toBe("value");
		expect(resolveConfigValue(success)).toBe("value");
		expect(readFileSync(counterFile, "utf-8").trim()).toBe("1");

		clearConfigValueCache();
		expect(resolveConfigValue(success)).toBe("value");
		expect(readFileSync(counterFile, "utf-8").trim()).toBe("2");

		const failure =
			'!sh -c \'count=$(cat "$PI_TEST_COUNTER_FILE"); echo $((count + 1)) > "$PI_TEST_COUNTER_FILE"; exit 1\'';
		expect(resolveConfigValue(failure)).toBeUndefined();
		expect(resolveConfigValue(failure)).toBeUndefined();
		expect(readFileSync(counterFile, "utf-8").trim()).toBe("3");
	});

	test("does not cache environment values", () => {
		process.env.TEST_CONFIG_DYNAMIC = "first";
		try {
			expect(resolveConfigValue("$TEST_CONFIG_DYNAMIC")).toBe("first");
			process.env.TEST_CONFIG_DYNAMIC = "second";
			expect(resolveConfigValue("$TEST_CONFIG_DYNAMIC")).toBe("second");
		} finally {
			delete process.env.TEST_CONFIG_DYNAMIC;
		}
	});

	test("uncached resolution executes a command on every call", () => {
		const counterFile = join(tempDir, "uncached-counter's $literal");
		writeFileSync(counterFile, "0");
		vi.stubEnv(
			"PI_TEST_COUNTER_FILE",
			process.platform === "win32" ? counterFile.replaceAll("\\", "/") : counterFile,
		);
		const command =
			'!sh -c \'count=$(cat "$PI_TEST_COUNTER_FILE"); echo $((count + 1)) > "$PI_TEST_COUNTER_FILE"; echo value\'';
		expect(resolveConfigValueUncached(command)).toBe("value");
		expect(resolveConfigValueUncached(command)).toBe("value");
		expect(readFileSync(counterFile, "utf-8").trim()).toBe("2");
	});

	test("uses stdin when the configured Windows shell requires it", () => {
		if (process.platform === "win32") return;
		const platformDescriptor = Object.getOwnPropertyDescriptor(process, "platform");
		vi.spyOn(shellModule, "getShellConfig").mockReturnValue({
			shell: "/bin/bash",
			args: ["-s"],
			commandTransport: "stdin",
		});
		try {
			Object.defineProperty(process, "platform", { configurable: true, value: "win32" });
			const expansion = "$" + "{name}";
			expect(resolveConfigValueUncached(`!name='World'; echo "Hello, ${expansion}!"`)).toBe("Hello, World!");
		} finally {
			if (platformDescriptor) Object.defineProperty(process, "platform", platformDescriptor);
		}
	});
});

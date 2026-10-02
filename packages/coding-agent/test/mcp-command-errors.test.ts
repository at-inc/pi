import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { InMemoryAuthStorageBackend } from "../src/core/auth-storage.ts";
import { runMcpCommand } from "../src/extensions/mcp/cli.ts";
import { McpOAuthCredentialStore } from "../src/extensions/mcp/oauth.ts";
import { startOAuthMcpServer } from "./suite/mcp-oauth-server.ts";

describe("MCP command credential errors", () => {
	const cleanups: (() => Promise<void> | void)[] = [];

	afterEach(async () => {
		vi.restoreAllMocks();
		while (cleanups.length > 0) await cleanups.pop()?.();
	});

	async function setup(clientSecret: string, refresh: boolean, failDiscovery = false) {
		const server = await startOAuthMcpServer();
		cleanups.push(server.close);
		const agentDir = mkdtempSync(join(tmpdir(), "pi-mcp-credential-errors-"));
		cleanups.push(() => rmSync(agentDir, { recursive: true, force: true }));
		writeFileSync(
			join(agentDir, "mcp.json"),
			JSON.stringify({
				mcpServers: {
					private: {
						url: server.url,
						oauth: {
							clientId: "test-client",
							clientSecret,
							...(failDiscovery ? { authServerMetadataUrl: new URL("/missing", server.url).href } : {}),
						},
					},
				},
			}),
		);
		const credentials = new McpOAuthCredentialStore(new InMemoryAuthStorageBackend());
		if (refresh) {
			await credentials.forServer("private", server.url).save({
				serverUrl: server.url,
				tokens: {
					access_token: "CODEQL_FAKE_ACCESS_TOKEN",
					refresh_token: "CODEQL_FAKE_REFRESH_TOKEN",
					token_type: "Bearer",
				},
			});
		}
		const output: string[] = [];
		vi.spyOn(console, "log").mockImplementation((line: string) => output.push(line));
		vi.spyOn(console, "error").mockImplementation((line: string) => output.push(line));
		const openUrl = vi.fn();
		return { options: { cwd: agentDir, agentDir, credentials, openUrl }, output, openUrl, serverUrl: server.url };
	}

	it.each([
		{ args: ["login", "private"], refresh: false },
		{ args: ["login", "private"], refresh: true },
		{ args: ["list"], refresh: true },
		{ args: ["list", "--json"], refresh: true },
	])("does not log failed secret commands for $args with refresh=$refresh", async ({ args, refresh }) => {
		vi.stubEnv("PI_TEST_MCP_STDOUT", "CODEQL_FAKE_STDOUT_SECRET");
		vi.stubEnv("PI_TEST_MCP_STDERR", "CODEQL_FAKE_STDERR_SECRET");
		const command = '!echo "$PI_TEST_MCP_STDOUT"; echo "$PI_TEST_MCP_STDERR" >&2; false CODEQL_FAKE_INLINE_SECRET';
		const { options, output, openUrl } = await setup(command, refresh);

		expect(await runMcpCommand(args, options)).toBe(1);

		const text = output.join("\n");
		expect(text).not.toContain("CODEQL_FAKE_");
		expect(text).not.toContain(command.slice(1));
		expect(text).not.toContain("PI_TEST_MCP_STDOUT");
		expect(text).not.toContain("PI_TEST_MCP_STDERR");
		expect(text).toContain("oauth.clientSecret from shell command");
		expect(openUrl).not.toHaveBeenCalled();
	});

	it.each(["CODEQL_FAKE_LITERAL_SECRET", "!echo CODEQL_FAKE_RESOLVED_SECRET"])(
		"retains safe OAuth failure diagnostics without logging %s",
		async (clientSecret) => {
			const { options, output, openUrl, serverUrl } = await setup(clientSecret, false, true);

			expect(await runMcpCommand(["login", "private"], options)).toBe(1);

			expect(output.join("\n")).toBe(
				`Sign-in to MCP server "private" failed: HTTP 404 loading authorization server metadata from ${new URL("/missing", serverUrl)}`,
			);
			expect(output.join("\n")).not.toContain("CODEQL_FAKE_");
			expect(openUrl).not.toHaveBeenCalled();
		},
	);
});

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtemp, readdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { createModels, fauxAssistantMessage, fauxProvider } from "@at-inc/pi-ai";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { type ConversationId, createRegistry, Harness } from "@earendil-works/pi-durable";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.ts";
import { ModelRuntime } from "../src/core/model-runtime.ts";
import { SessionManager } from "../src/core/session-manager.ts";
import { SettingsManager } from "../src/core/settings-manager.ts";
import { importV3Session } from "../src/experimental/durable/import.ts";
import * as legacy from "../src/experimental/durable/legacy-v3.ts";
import { agentOf, openDurable } from "../src/experimental/durable/runtime.ts";
import { durableSessionsDirectory, selectSession } from "../src/experimental/durable/sessions.ts";

let directory: string;
let source: string;
let bytes: string;

beforeEach(async () => {
	directory = await mkdtemp(join(tmpdir(), "pi-durable-import-"));
	vi.stubEnv("PI_CODING_AGENT_DIR", join(directory, "agent"));
	source = join(directory, "source.jsonl");
	const timestamp = "2026-09-01T00:00:00.000Z";
	bytes = `${[
		{ type: "session", version: 3, id: "source-session", timestamp, cwd: directory },
		{
			type: "message",
			id: "user-1",
			parentId: null,
			timestamp,
			message: { role: "user", content: "Shared question", timestamp: 1 },
		},
		{
			type: "message",
			id: "assistant-1",
			parentId: "user-1",
			timestamp,
			message: fauxAssistantMessage("First branch", { timestamp: 2 }),
		},
		{
			type: "message",
			id: "assistant-2",
			parentId: "user-1",
			timestamp,
			message: fauxAssistantMessage("Selected branch", { timestamp: 3 }),
		},
	]
		.map((entry) => JSON.stringify(entry))
		.join("\n")}\n`;
	await writeFile(source, bytes);
});

afterEach(async () => {
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
	await rm(directory, { recursive: true, force: true });
});

test("publishes a private SQLite session and preserves stable v3 continuation", async () => {
	const before = SessionManager.open(source).buildSessionContext();
	const imported = await importV3Session(source, directory);
	expect(imported.reused).toBe(false);
	expect(imported.manifest.source).toEqual({
		path: source,
		sha256: createHash("sha256").update(bytes).digest("hex"),
		sessionId: "source-session",
	});
	expect(imported.manifest.branches).toHaveLength(2);
	expect(await readFile(join(imported.directory, "source.jsonl"), "utf8")).toBe(bytes);
	expect(JSON.parse(await readFile(join(imported.directory, "import.json"), "utf8"))).toEqual(imported.manifest);
	if (process.platform !== "win32") {
		expect((await stat(imported.directory)).mode & 0o777).toBe(0o700);
		expect((await stat(join(imported.directory, "source.jsonl"))).mode & 0o777).toBe(0o600);
	}
	const runtime = await ModelRuntime.create({
		credentials: AuthStorage.inMemory(),
		modelsPath: null,
		refreshOnCreate: false,
	});
	vi.spyOn(ModelRuntime, "create").mockResolvedValue(runtime);
	vi.spyOn(SettingsManager, "create").mockReturnValue(SettingsManager.inMemory());
	const opened = await openDurable({ cwd: directory, sessionId: imported.sessionId });
	try {
		expect(agentOf(opened.view.current().conversation).model).toEqual({ provider: "faux", modelId: "faux-1" });
		expect(opened.view.current().conversations.map((conversation) => conversation.label)).toEqual([
			"main",
			expect.stringMatching(/^branch /),
		]);
		expect(opened.view.current().conversation.entries.flatMap((entry) => entry.model ?? [])).toContainEqual(
			fauxAssistantMessage("Selected branch", { timestamp: 3 }),
		);
	} finally {
		await opened.close();
	}
	expect(SessionManager.open(source).buildSessionContext()).toEqual(before);
	expect(await readFile(source, "utf8")).toBe(bytes);
});

test("reuses the same snapshot without overwriting subsequent durable entries", async () => {
	const imported = await importV3Session(source, directory);
	const harness = await Harness.open(
		await openNodeSqliteStorage(join(imported.directory, "session.sqlite")),
		{ models: createModels(), registry: createRegistry() },
		BACKGROUND_CONTEXT,
	);
	try {
		const root = await harness.root(BACKGROUND_CONTEXT);
		await harness.commit(
			(tx) => tx.appendEntry(root.id, { kind: "test.after-import", data: { keep: true } }),
			BACKGROUND_CONTEXT,
		);
	} finally {
		await harness.close(BACKGROUND_CONTEXT);
	}
	const database = await readFile(join(imported.directory, "session.sqlite"));
	const repeated = await importV3Session(source, directory);
	expect(repeated.reused).toBe(true);
	expect(repeated.sessionId).toBe(imported.sessionId);
	expect(await readFile(join(imported.directory, "session.sqlite"))).toEqual(database);
});

test("continues each imported branch with only its own history after reopening SQLite", async () => {
	const imported = await importV3Session(source, directory);
	const faux = fauxProvider();
	const models = createModels();
	models.setProvider(faux.provider);
	const harness = await Harness.open(
		await openNodeSqliteStorage(join(imported.directory, "session.sqlite")),
		{ models, registry: createRegistry() },
		BACKGROUND_CONTEXT,
	);
	try {
		harness.resume();
		for (const branch of imported.manifest.branches) {
			const original = branch.leafId === "assistant-1" ? "First branch" : "Selected branch";
			const other = branch.leafId === "assistant-1" ? "Selected branch" : "First branch";
			let called = false;
			faux.setResponses([
				(request) => {
					called = true;
					expect(request.messages).toContainEqual(
						fauxAssistantMessage(original, {
							timestamp: branch.leafId === "assistant-1" ? 2 : 3,
						}),
					);
					expect(JSON.stringify(request.messages)).not.toContain(other);
					return fauxAssistantMessage(`Continued ${original}`);
				},
			]);
			const conversation = (await harness.conversation(
				branch.conversationId as ConversationId,
				BACKGROUND_CONTEXT,
			))!;
			const submission = await conversation.submit({ type: "input", content: "Continue here" }, BACKGROUND_CONTEXT);
			expect(await submission.wait(BACKGROUND_CONTEXT)).toMatchObject({ status: "done" });
			expect(called).toBe(true);
			expect(JSON.stringify((await conversation.context(BACKGROUND_CONTEXT)).messages)).toContain(
				`Continued ${original}`,
			);
		}
	} finally {
		await harness.close(BACKGROUND_CONTEXT);
	}
	expect(await readFile(source, "utf8")).toBe(bytes);
});

test("serializes concurrent imports of the same snapshot", async () => {
	const results = await Promise.all([importV3Session(source, directory), importV3Session(source, directory)]);
	expect(results[0].sessionId).toBe(results[1].sessionId);
	expect(results.filter((result) => result.reused)).toHaveLength(1);
});

test("imports changed source contents as a new snapshot", async () => {
	const first = await importV3Session(source, directory);
	await writeFile(source, bytes.replace("Selected branch", "Updated branch"));
	const second = await importV3Session(source, directory);
	expect(second.reused).toBe(false);
	expect(second.sessionId).not.toBe(first.sessionId);
	expect(await readFile(join(first.directory, "source.jsonl"), "utf8")).toBe(bytes);
});

test("rejects a damaged snapshot without exposing a partial durable session", async () => {
	await writeFile(source, `${bytes}{"type":`);
	await expect(importV3Session(source, directory)).rejects.toThrow();
	await expect(stat(durableSessionsDirectory(directory))).rejects.toMatchObject({ code: "ENOENT" });
	expect(await readFile(source, "utf8")).toBe(`${bytes}{"type":`);
});

test("removes staging and releases the import lock after a converter failure", async () => {
	const failure = vi.spyOn(legacy, "importLegacyV3").mockRejectedValueOnce(new Error("import failed"));
	await expect(importV3Session(source, directory)).rejects.toThrow("import failed");
	expect(await readdir(durableSessionsDirectory(directory))).toEqual([]);
	failure.mockRestore();
	await expect(importV3Session(source, directory)).resolves.toMatchObject({ reused: false });
});

test("rejects invalid UTF-8 instead of replacing source text", async () => {
	const invalid = Buffer.concat([Buffer.from(bytes), Buffer.from([0xff])]);
	await writeFile(source, invalid);
	await expect(importV3Session(source, directory)).rejects.toThrow();
	await expect(stat(durableSessionsDirectory(directory))).rejects.toMatchObject({ code: "ENOENT" });
	expect(await readFile(source)).toEqual(invalid);
});

test("refuses to reuse an import with a changed source archive", async () => {
	const imported = await importV3Session(source, directory);
	await writeFile(join(imported.directory, "source.jsonl"), "damaged");
	await expect(importV3Session(source, directory)).rejects.toThrow("archive does not match");
});

test("imports through the source CLI without opening a TUI or calling a provider", async () => {
	const resolver = fileURLToPath(new URL("../src/experimental/source-resolver.ts", import.meta.url));
	const main = fileURLToPath(new URL("../src/experimental/durable/main.ts", import.meta.url));
	const result = spawnSync(process.execPath, ["--import", resolver, main, "--import", source], {
		cwd: directory,
		encoding: "utf8",
		timeout: 20_000,
		env: { PATH: process.env.PATH, HOME: directory, PI_CODING_AGENT_DIR: join(directory, "agent"), PI_OFFLINE: "1" },
	});
	expect(result.error).toBeUndefined();
	expect(result.status, result.stderr).toBe(0);
	const output = JSON.parse(result.stdout) as { sessionId: string };
	const location = await selectSession(directory, false, output.sessionId);
	await location.release();
	expect(await readFile(source, "utf8")).toBe(bytes);
});

test("rejects ambiguous selection and path-shaped session IDs", async () => {
	await expect(selectSession(directory, false, "../outside")).rejects.toThrow("Invalid durable session ID");
	const imported = await importV3Session(source, directory);
	await expect(selectSession(directory, true, imported.sessionId)).rejects.toThrow("either --session or --continue");
});

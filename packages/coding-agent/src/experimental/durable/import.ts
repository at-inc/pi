import { createHash, randomUUID } from "node:crypto";
import { lstat, mkdir, mkdtemp, readdir, readFile, realpath, rename, rm, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createModels } from "@at-inc/pi-ai";
import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context";
import { createRegistry, Harness } from "@earendil-works/pi-durable";
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node";
import lockfile from "proper-lockfile";
import { importLegacyV3, type LegacyV3ImportResult, parseLegacyV3 } from "./legacy-v3.ts";
import { DURABLE_SESSION_ID, durableSessionsDirectory } from "./sessions.ts";

export interface DurableImportManifest extends LegacyV3ImportResult {
	format: "pi-durable-v3-import";
	version: 1;
	source: { path: string; sha256: string; sessionId: string };
	importedAt: string;
	cwd: string;
	sessionId: string;
}

export interface DurableImportResult {
	sessionId: string;
	directory: string;
	reused: boolean;
	manifest: DurableImportManifest;
}

export async function importV3Session(sourcePath: string, cwdInput = process.cwd()): Promise<DurableImportResult> {
	const source = await realpath(resolve(sourcePath));
	const bytes = await readFile(source);
	const legacy = parseLegacyV3(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
	const sha256 = createHash("sha256").update(bytes).digest("hex");
	const cwd = await realpath(resolve(cwdInput));
	const root = durableSessionsDirectory(cwd);
	await mkdir(root, { recursive: true, mode: 0o700 });
	const release = await lockfile.lock(root, {
		realpath: false,
		retries: { retries: 12, minTimeout: 1000, maxTimeout: 1000 },
	});
	let staging: string | undefined;
	try {
		for (const entry of await readdir(root, { withFileTypes: true })) {
			if (!entry.isDirectory() || !DURABLE_SESSION_ID.test(entry.name)) continue;
			const directory = join(root, entry.name);
			let content: string;
			try {
				content = await readFile(join(directory, "import.json"), "utf8");
			} catch (error) {
				if ((error as NodeJS.ErrnoException).code === "ENOENT") continue;
				throw error;
			}
			const manifest = JSON.parse(content) as DurableImportManifest;
			if (manifest.format !== "pi-durable-v3-import" || manifest.source?.sha256 !== sha256) continue;
			if (manifest.version !== 1 || manifest.cwd !== cwd || manifest.sessionId !== entry.name) {
				throw new Error(`Invalid existing import manifest: ${directory}`);
			}
			if (!(await lstat(join(directory, "session.sqlite"))).isFile()) {
				throw new Error(`Imported session database is missing: ${directory}`);
			}
			if (!(await readFile(join(directory, "source.jsonl"))).equals(bytes)) {
				throw new Error(`Imported source archive does not match: ${directory}`);
			}
			return { sessionId: entry.name, directory, reused: true, manifest };
		}
		staging = await mkdtemp(join(root, ".import-"));
		await writeFile(join(staging, "source.jsonl"), bytes, { flag: "wx", mode: 0o600 });
		const harness = await Harness.open(
			await openNodeSqliteStorage(join(staging, "session.sqlite")),
			{ models: createModels(), registry: createRegistry() },
			BACKGROUND_CONTEXT,
		);
		let converted: LegacyV3ImportResult;
		try {
			converted = await importLegacyV3(legacy, harness, cwd);
		} finally {
			await harness.close(BACKGROUND_CONTEXT);
		}
		const sessionId = `${String(Date.now()).padStart(13, "0")}-${randomUUID()}`;
		const manifest: DurableImportManifest = {
			...converted,
			format: "pi-durable-v3-import",
			version: 1,
			source: { path: source, sha256, sessionId: legacy.header.id },
			importedAt: new Date().toISOString(),
			cwd,
			sessionId,
		};
		await writeFile(join(staging, "import.json"), `${JSON.stringify(manifest, null, 2)}\n`, {
			flag: "wx",
			mode: 0o600,
		});
		const directory = join(root, sessionId);
		await rename(staging, directory);
		staging = undefined;
		return { sessionId, directory, reused: false, manifest };
	} finally {
		try {
			if (staging !== undefined) await rm(staging, { recursive: true, force: true });
		} finally {
			await release();
		}
	}
}

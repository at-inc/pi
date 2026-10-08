import { createHash, randomUUID } from "node:crypto";
import { lstat, mkdir, readdir, realpath } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import lockfile from "proper-lockfile";
import { getAgentDir } from "../../config.ts";

export const DURABLE_SESSION_ID = /^\d{13}-[0-9a-f-]{36}$/u;

export function durableSessionsDirectory(cwd: string): string {
	return join(
		getAgentDir(),
		"experimental",
		"durable-sessions",
		createHash("sha256").update(cwd).digest("hex").slice(0, 24),
	);
}

/** One session directory holding `session.sqlite`, locked by this process. */
export interface SessionLocation {
	id: string;
	directory: string;
	database: string;
	cwd: string;
	created: boolean;
	release(): Promise<void>;
}

/** A new session for `cwd`, or its newest one with `continueSession`. */
export async function selectSession(
	cwdInput: string,
	continueSession: boolean,
	sessionId?: string,
): Promise<SessionLocation> {
	if (sessionId !== undefined && !DURABLE_SESSION_ID.test(sessionId)) throw new Error("Invalid durable session ID");
	if (sessionId !== undefined && continueSession) throw new Error("Use either --session or --continue, not both");
	const cwd = await realpath(resolve(cwdInput));
	const root = durableSessionsDirectory(cwd);
	await mkdir(root, { recursive: true });

	let directory: string;
	let created = false;
	if (sessionId !== undefined) {
		directory = join(root, sessionId);
		if (!(await lstat(directory)).isDirectory()) throw new Error(`Not a durable session directory: ${directory}`);
		if (!(await lstat(join(directory, "session.sqlite"))).isFile()) {
			throw new Error(`No durable session database: ${directory}`);
		}
	} else if (continueSession) {
		const entries = await readdir(root, { withFileTypes: true });
		const newest = entries
			.filter((entry) => entry.isDirectory() && DURABLE_SESSION_ID.test(entry.name))
			.map((entry) => entry.name)
			.sort()
			.at(-1);
		if (!newest) throw new Error(`No durable session exists for ${cwd}`);
		directory = join(root, newest);
	} else {
		directory = join(root, `${String(Date.now()).padStart(13, "0")}-${randomUUID()}`);
		await mkdir(directory);
		created = true;
	}

	let release: () => Promise<void>;
	try {
		// A lock left by a crashed process goes stale after 10 s; wait that long before giving up.
		release = await lockfile.lock(directory, {
			realpath: false,
			retries: { retries: 12, minTimeout: 1000, maxTimeout: 1000 },
		});
	} catch (error) {
		throw new Error(`Session is already open in another process: ${directory}`, { cause: error });
	}
	return { id: basename(directory), directory, database: join(directory, "session.sqlite"), cwd, created, release };
}

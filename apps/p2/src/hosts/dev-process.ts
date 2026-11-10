// Linux process identities used by the dev supervisor to reap only its reported worker groups.
import { readFileSync } from "node:fs";

export type WorkerGroup = { readonly pid: number; readonly started: string };

/** Records a detached worker only while it is a direct child of the reporting server. */
export function workerGroup(pid: number, parent: number): WorkerGroup | undefined {
	const identity = processIdentity(pid);
	return identity?.parent === parent && identity.group === pid ? { pid, started: identity.started } : undefined;
}

/** Whether the recorded worker still leads the same process group, even after its parent exits. */
export function isWorkerGroup(worker: WorkerGroup): boolean {
	const identity = processIdentity(worker.pid);
	return identity?.group === worker.pid && identity.started === worker.started;
}

function processIdentity(pid: number): { parent: number; group: number; started: string } | undefined {
	try {
		const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
		const fields = stat.slice(stat.lastIndexOf(") ") + 2).split(" ");
		return { parent: Number(fields[1]), group: Number(fields[2]), started: fields[19]! };
	} catch {
		return undefined;
	}
}

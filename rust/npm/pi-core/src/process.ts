// Child processes of the Node host, as the MCP stdio transport of the TS package spawns them: cross-spawn with piped
// stdin and stdout, each process in its own group on Unix so killing the group reaches its children, taskkill on
// Windows, and an exit hook that terminates the groups still running when Node exits.

import { type ChildProcess, spawn } from "node:child_process";
import process from "node:process";
import crossSpawn from "cross-spawn";
import type { PiChildProcess, PiProcessListeners, PiSpawnOptions } from "../wasm/pi_wasm.js";

const USE_PROCESS_GROUPS = process.platform !== "win32";

/** Process groups of running processes, killed if Node exits without closing them. */
const liveProcessGroups = new Set<number>();
let exitHookInstalled = false;

/** `PiHost.spawnProcess` on Node. */
export function spawnProcess(options: PiSpawnOptions, listeners: PiProcessListeners): Promise<PiChildProcess> {
	const env = options.clearEnv ? { ...options.env } : { ...process.env, ...options.env };
	const child = crossSpawn(options.command, options.args, {
		cwd: options.cwd,
		env,
		stdio: ["pipe", "pipe", options.inheritStderr ? "inherit" : "pipe"],
		windowsHide: true,
		detached: USE_PROCESS_GROUPS,
	});
	const pid = child.pid;
	if (USE_PROCESS_GROUPS && pid !== undefined) {
		installExitHook();
		liveProcessGroups.add(pid);
		child.once("exit", () => liveProcessGroups.delete(pid));
	}
	child.stdout?.on("data", (chunk: Buffer) => listeners.onStdout(chunk));
	child.stdout?.on("error", (error) => listeners.onError(error));
	// A failed write rejects its `write` call, which the caller reports; the event only needs a listener.
	child.stdin?.on("error", () => {});
	child.stderr?.on("data", (chunk: Buffer) => listeners.onStderr(chunk));
	child.stderr?.on("error", (error) => listeners.onError(error));
	child.on("close", () => listeners.onClose());
	return new Promise((resolve, reject) => {
		const onSpawn = () => {
			child.off("error", onError);
			child.on("error", (error) => listeners.onError(error));
			resolve(childProcess(child));
		};
		const onError = (error: Error) => {
			child.off("spawn", onSpawn);
			reject(error);
		};
		child.once("spawn", onSpawn);
		child.once("error", onError);
	});
}

function childProcess(child: ChildProcess): PiChildProcess {
	return {
		get pid() {
			return child.pid;
		},
		write: (data) =>
			new Promise<void>((resolve, reject) => {
				const stdin = child.stdin;
				if (!stdin) {
					reject(Object.assign(new Error("write EPIPE"), { code: "EPIPE" }));
					return;
				}
				stdin.write(data, (error) => (error ? reject(error) : resolve()));
			}),
		endStdin: () => child.stdin?.end(),
		get stdinWritable() {
			return child.stdin?.writable ?? false;
		},
		get exited() {
			return child.exitCode !== null || child.signalCode !== null;
		},
		kill: (signal) => killProcessTree(child, signal),
	};
}

function killProcessTree(child: ChildProcess, signal: NodeJS.Signals): void {
	const pid = child.pid;
	if (process.platform === "win32" && pid !== undefined) {
		if (child.exitCode !== null) return;
		// Windows has no graceful signals, and cross-spawn runs `.cmd` shims through cmd.exe; killing only
		// that would leave the process running.
		spawn("taskkill", ["/pid", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true }).on(
			"error",
			() => {},
		);
		return;
	}
	if (USE_PROCESS_GROUPS && pid !== undefined) {
		try {
			// Negative pid: the whole group, so wrappers like `npx` or `uvx` do not leave the server behind.
			process.kill(-pid, signal);
			return;
		} catch {
			// The group is gone or was never created; fall back to the direct child.
		}
	}
	try {
		child.kill(signal);
	} catch {}
}

function installExitHook(): void {
	if (exitHookInstalled) return;
	exitHookInstalled = true;
	process.once("exit", () => {
		for (const pid of liveProcessGroups) {
			try {
				process.kill(-pid, "SIGTERM");
			} catch {}
		}
	});
}

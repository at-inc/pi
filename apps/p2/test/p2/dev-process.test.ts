import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { test } from "node:test";
import { isWorkerGroup, workerGroup } from "../../src/hosts/dev-process.ts";

test(
	"dev worker groups require the reporting parent, group leader and original process start time",
	{ skip: process.platform !== "linux" },
	async (t) => {
		const worker = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], {
			detached: true,
			stdio: "ignore",
		});
		await once(worker, "spawn");
		t.after(async () => {
			if (worker.exitCode !== null || worker.signalCode !== null) return;
			const exited = once(worker, "exit");
			worker.kill("SIGKILL");
			await exited;
		});
		assert(worker.pid);
		const group = workerGroup(worker.pid, process.pid);
		assert(group);
		assert.equal(isWorkerGroup(group), true);
		assert.equal(workerGroup(worker.pid, 0), undefined, "another server cannot claim this worker");
		assert.equal(
			isWorkerGroup({ ...group, started: String(BigInt(group.started) + 1n) }),
			false,
			"a reused PID has another start time",
		);
		const exited = once(worker, "exit");
		worker.kill("SIGKILL");
		await exited;
		assert.equal(isWorkerGroup(group), false, "an exited process is no longer owned");
	},
);

test(
	"dev worker groups reject a child that does not lead its process group",
	{ skip: process.platform !== "linux" },
	async (t) => {
		const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });
		await once(child, "spawn");
		t.after(async () => {
			const exited = once(child, "exit");
			child.kill("SIGKILL");
			await exited;
		});
		assert(child.pid);
		assert.equal(workerGroup(child.pid, process.pid), undefined);
	},
);

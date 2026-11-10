import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { constant } from "@p2/app/v1";
import filesServer from "../../plugins/files/server.ts";
import { MAX_UPLOAD_BYTES, UploadError } from "../../plugins/files/uploads.ts";
import { Catalog } from "../../plugins/sessions/contract.ts";
import workspaceServer from "../../plugins/workspace/server.ts";
import { devControl } from "../../src/hosts/dev.ts";
import { sendDevCommand, serveDevChannel } from "../../src/hosts/dev-channel.ts";
import { createP2Plugins } from "../../src/hosts/plugins.ts";
import { Routes } from "../../src/runtime/server.ts";
import { APP, http, startFixture, TEST, testHost } from "./helpers.ts";

const stack = "Error: private failure\n    at privateFunction (/private/server.ts:42:7)";
const error = Object.assign(new Error("Useful failure message"), {
	stack,
	toString: () => stack,
	toJSON: () => ({ stack }),
});
const thrownObject = { stack, toString: () => stack, toJSON: () => ({ stack }) };

test("dev command errors return messages without coercing or serializing thrown objects", async (t) => {
	const directory = await mkdtemp(join(tmpdir(), "p2-http-errors-"));
	let failure: unknown = error;
	const close = await serveDevChannel(directory, async () => {
		throw failure;
	});
	t.after(async () => {
		await close();
		await rm(directory, { recursive: true, force: true });
	});
	for (const [value, message] of [
		[error, error.message],
		[thrownObject, "Dev command failed"],
		[null, "Dev command failed"],
	] as const) {
		failure = value;
		assert.deepEqual(await sendDevCommand(directory, { action: "reload" }), { ok: false, message });
	}
});

test("file routes preserve error messages, status codes and limits without exposing thrown objects", async (t) => {
	let failure: unknown = error;
	const unused = () => assert.fail("The failing server root must not call the catalog");
	const fixture = await testHost({
		facets: [filesServer, workspaceServer],
		stubs: [[Catalog, { sessions: constant([]), get: unused, create: unused, activity: unused, update: unused }]],
		server: {
			root: async () => {
				throw failure;
			},
		},
	});
	t.after(() => fixture.close());
	const routes = (await fixture.host.registry(Routes, TEST)).value;
	const server = createServer((request, response) => {
		const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
		const route = routes.get(`${request.method} ${pathname}`);
		assert(route);
		void fixture
			.root(async (ctx) =>
				route(
					request,
					response,
					{
						principal: { kind: "owner", access: "local" },
						origin: { origin: "http://localhost", remote: false, port: 80 },
					},
					ctx,
				),
			)
			.catch(() => response.destroy());
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	t.after(() => new Promise<void>((resolve) => server.close(() => resolve())));
	const address = server.address();
	assert(address && typeof address !== "string");
	for (const [value, message, status] of [
		[error, error.message, 400],
		[new UploadError(413, "File too large"), "File too large", 413],
		[thrownObject, "Upload failed", 400],
	] as const) {
		failure = value;
		const response = await http(address.port, "/upload?session=test&name=test.txt", {
			method: "POST",
			headers: { Origin: "http://localhost" },
		});
		assert.equal(response.status, status);
		assert.equal(response.headers["content-type"], "application/json");
		assert.deepEqual(JSON.parse(response.body), { error: message, limit: MAX_UPLOAD_BYTES });
	}
	for (const [value, message] of [
		[error, error.message],
		[thrownObject, "Workspace request failed"],
	] as const) {
		failure = value;
		const response = await http(address.port, "/workspace-file?session=test&path=test.txt");
		assert.equal(response.status, 404);
		assert.deepEqual(JSON.parse(response.body), { error: message });
	}
});

test("reload status keeps explicit messages without coercing arbitrary exceptions", async (t) => {
	const plugins = createP2Plugins(undefined, () => {});
	t.after(() => plugins.close());
	let failure: unknown = error;
	const control = devControl({
		plugins: plugins.plugins,
		runtimeDirectory: APP,
		setChanges: () => [],
		reload: async () => {
			throw failure;
		},
	});
	t.after(() => control.close());
	for (const [value, message] of [
		[error, error.message],
		[thrownObject, "Plugin reload failed"],
	] as const) {
		failure = value;
		await assert.rejects(control.reload(), (caught: unknown) => caught === value);
		assert.equal(control.status().phase, "error");
		assert.equal(control.status().message, `Reload refused: ${message}`);
	}
});

test("public dev responses preserve validation messages and refusal status", async (t) => {
	const fixture = await startFixture({ plugins: ["devices"], dev: false });
	t.after(() => fixture.close());
	for (const [body, status, message] of [
		["{", 400, "Invalid dev command JSON"],
		[JSON.stringify({ action: "invalid" }), 400, "Invalid dev action"],
		[JSON.stringify({ action: "reload" }), 409, "Dev mode is not enabled"],
	] as const) {
		const response = await fixture.http("/__p2_dev", {
			method: "POST",
			headers: { Origin: `http://127.0.0.1:${fixture.server.port}` },
			body,
		});
		assert.equal(response.status, status);
		assert.deepEqual(JSON.parse(response.body), { ok: false, message });
	}
});

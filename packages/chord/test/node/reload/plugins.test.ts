// The reload counter fixture preserves code labels as data in both contract versions.

import { describe, expect, it } from "vitest";
import { BACKGROUND_CONTEXT as context } from "../../../src/context/index.ts";
import * as chord from "../../../src/index.ts";
import { hostKind, int, method, remote, str } from "../../../src/index.ts";
import { testPlugin } from "../../../src/node/plugins/index.ts";
import { counterPlugin } from "../fixtures/reload/plugins.ts";
import { tempRoot, writePlugin } from "../plugins/support.ts";

const Server = hostKind("server");
const Counter = remote("reload.counter", {
	version: 1,
	params: { id: str() },
	hostAddress: () => Server(),
	variants: {
		main: {
			state: { label: str(), count: int() },
			methods: {
				add: method(int(), str()),
				slow: method(str(), str()),
				slowAdd: method(str(), str()),
				code: method(undefined, str()),
				visits: method(undefined, int()),
			},
		},
	},
});

describe.each(["v1", "v2"] as const)("counter fixture labels with contract %s", (contract) => {
	it.each(["v1", `\${(() => { throw new Error("label executed"); })()}`, '`quoted`\\label"\nnext line'])(
		"preserves %j literally in state and method results",
		async (code) => {
			const dir = await writePlugin(
				await tempRoot(),
				"counter",
				"@test/counter",
				counterPlugin({ code, placement: "server", contract, accepts: true }),
			);
			const plugin = await testPlugin(
				dir,
				{
					kind: "server",
					api: { name: "@test/app", versions: ["v1"] },
					modules: { "@test/app/v1": { ...chord } },
					address: Server(),
					principal: { kind: "owner" },
				},
				context,
			);
			try {
				const counter = await plugin.host.get(Counter, { id: "p1" }, context);
				expect(counter.label.value).toBe(`${code}:0`);
				expect(await counter.code(context)).toBe(code);
				expect(await counter.add(2, context)).toBe(`${code}:2`);
				expect(counter.label.value).toBe(`${code}:2`);
				expect(await counter.visits(context)).toBe(1);
			} finally {
				await plugin.close(context);
			}
		},
	);
});

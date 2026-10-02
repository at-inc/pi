import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import { workspaceSourcePaths } from "../../vitest.base.ts";

const durableSrcIndex = fileURLToPath(new URL("./src/index.ts", import.meta.url));
const durableSrcTesting = fileURLToPath(new URL("./src/testing/index.ts", import.meta.url));

export default defineConfig({
	test: {
		environment: "node",
	},
	resolve: {
		conditions: ["source"],
		alias: [
			{ find: /^@earendil-works\/pi-durable$/, replacement: durableSrcIndex },
			{ find: /^@earendil-works\/pi-durable\/testing$/, replacement: durableSrcTesting },
			{ find: /^@at-inc\/pi-ai$/, replacement: workspaceSourcePaths.aiIndex },
			{ find: /^@at-inc\/pi-ai\/models$/, replacement: workspaceSourcePaths.aiModels },
			{
				find: /^@at-inc\/pi-ai\/utils\/(.+)$/,
				replacement: `${workspaceSourcePaths.aiUtils}/$1.ts`,
			},
			{
				find: /^@at-inc\/pi-ai\/providers\/(.+)$/,
				replacement: `${workspaceSourcePaths.aiProviders}/$1.ts`,
			},
		],
	},
	ssr: { resolve: { conditions: ["source"] } },
});

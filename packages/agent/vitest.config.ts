import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

const telemetrySrcIndex = fileURLToPath(new URL("../telemetry/src/index.ts", import.meta.url));
const aiSrcIndex = fileURLToPath(new URL("../ai/src/index.ts", import.meta.url));
const aiSrcCompat = fileURLToPath(new URL("../ai/src/compat.ts", import.meta.url));
const agentSrcIndex = fileURLToPath(new URL("./src/index.ts", import.meta.url));

export default defineConfig({
	test: {
		globals: true,
		environment: "node",
		testTimeout: 30000, // 30 seconds for API calls
		reporters: process.env.GITHUB_ACTIONS ? ["dot", "github-actions"] : ["dot"],
		silent: "passed-only",
	},
	resolve: {
		conditions: ["source"],
		alias: [
			{ find: /^@earendil-works\/pi-telemetry$/, replacement: telemetrySrcIndex },
			{ find: /^@at-inc\/pi-agent-core$/, replacement: agentSrcIndex },
			{ find: /^@at-inc\/pi-ai$/, replacement: aiSrcIndex },
			{ find: /^@at-inc\/pi-ai\/compat$/, replacement: aiSrcCompat },
			{
				find: /^@at-inc\/pi-ai\/utils\/uuid$/,
				replacement: fileURLToPath(new URL("../ai/src/utils/uuid.ts", import.meta.url)),
			},
		],
	},
	ssr: { resolve: { conditions: ["source"] } },
});

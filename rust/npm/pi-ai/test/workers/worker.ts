// A Worker that runs one openai-completions stream through the facade, for test/workers/smoke.ts. The request body
// is the model's base URL; the response is the events' types and the final message.

import { streamSimple } from "../../src/compat.ts";

export default {
	async fetch(request: Request): Promise<Response> {
		const baseUrl = await request.text();
		const stream = streamSimple(
			{
				id: "m",
				name: "m",
				api: "openai-completions",
				provider: "custom",
				baseUrl,
				reasoning: false,
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 1000,
				maxTokens: 100,
			},
			{ messages: [{ role: "user", content: "hi", timestamp: Date.now() }] },
			{ apiKey: "test" },
		);
		const events: string[] = [];
		for await (const event of stream) events.push(event.type);
		return Response.json({ events, message: await stream.result() });
	},
};

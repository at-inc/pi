import assert from "node:assert/strict";
import { Agent, type AgentEvent, type AgentTool } from "@earendil-works/pi-agent-core";
import { createModels, Type } from "@earendil-works/pi-ai";
import { fauxAssistantMessage, fauxProvider, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";

const faux = fauxProvider();
faux.setResponses([
	fauxAssistantMessage([fauxToolCall("echo", { text: "beta-smoke" }, { id: "beta-call" })], { stopReason: "toolUse" }),
	fauxAssistantMessage("beta-ok"),
]);
const models = createModels();
models.setProvider(faux.provider);
const parameters = Type.Object({ text: Type.String() });
let executions = 0;
const tool: AgentTool<typeof parameters> = {
	name: "echo",
	label: "Echo",
	description: "Return the supplied text",
	parameters,
	async execute(_id, args) {
		executions++;
		return { content: [{ type: "text", text: args.text }], details: {} };
	},
};
const agent = new Agent({
	initialState: { model: faux.getModel(), systemPrompt: "Offline library smoke test", tools: [tool] },
	streamFn: models.streamSimple.bind(models),
});
const events: AgentEvent[] = [];
agent.subscribe((event) => {
	events.push(event);
});
await agent.prompt("Run the echo tool, then reply beta-ok.");
assert.equal(agent.state.errorMessage, undefined);
assert.equal(executions, 1);
assert.equal(faux.state.callCount, 2);
assert.ok(events.some((event) => event.type === "tool_execution_end"));
assert.ok(events.some((event) => event.type === "agent_end"));
const answer = agent.state.messages.findLast((message) => message.role === "assistant");
assert.ok(answer?.role === "assistant");
assert.deepEqual(answer.content, [{ type: "text", text: "beta-ok" }]);
console.log("Aliased library SDK smoke passed: one tool execution and two offline model turns.");

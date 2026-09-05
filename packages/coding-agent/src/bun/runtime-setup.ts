import { bedrockProviderModule } from "@at-inc/pi-ai/bedrock-provider";
import { registerBunOAuthFlows } from "@at-inc/pi-ai/bun-oauth";
import { setBedrockProviderModule } from "@at-inc/pi-ai/compat";
import { APP_NAME } from "../config.ts";

process.title = APP_NAME;
process.emitWarning = (() => {}) as typeof process.emitWarning;
registerBunOAuthFlows();
setBedrockProviderModule(bedrockProviderModule);

#!/usr/bin/env node

import { importV3Session } from "./import.ts";
import { type OpenDurableOptions, openDurable } from "./runtime.ts";
import { runDurableTui } from "./tui.ts";

function parseArgs(argv: readonly string[]): { options: OpenDurableOptions; importFile?: string } {
	let continueSession = false;
	let sessionId: string | undefined;
	let importFile: string | undefined;
	for (let index = 0; index < argv.length; index++) {
		const arg = argv[index];
		if (arg === "--continue" || arg === "-c") continueSession = true;
		else if (arg === "--session" || arg === "--import") {
			const value = argv[++index];
			if (!value || value.startsWith("--")) throw new Error(`${arg} needs a value`);
			if (arg === "--session") sessionId = value;
			else importFile = value;
		} else throw new Error(`Unknown argument: ${arg}`);
	}
	if (Number(continueSession) + Number(sessionId !== undefined) + Number(importFile !== undefined) > 1) {
		throw new Error("Use only one of --continue, --session, or --import");
	}
	return { options: { continueSession, sessionId }, importFile };
}

const command = parseArgs(process.argv.slice(2));
if (command.importFile !== undefined) {
	const imported = await importV3Session(command.importFile);
	console.log(
		JSON.stringify(
			{
				sessionId: imported.sessionId,
				directory: imported.directory,
				reused: imported.reused,
				branches: imported.manifest.branches,
				warnings: imported.manifest.warnings,
			},
			null,
			2,
		),
	);
} else {
	const durable = await openDurable(command.options);
	try {
		await runDurableTui(durable.view, durable.controller, durable.settings);
	} finally {
		await durable.close();
	}
}

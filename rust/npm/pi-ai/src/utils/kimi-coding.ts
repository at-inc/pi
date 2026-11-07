// utils/kimi-coding: endpoints in the Rust core. Selecting them at login needs the auth interaction, which belongs to
// the pi-ai auth slice.

import type * as ts from "@at-inc/pi-ai/utils/kimi-coding";
import { wasm } from "@at-inc/pi-core";
import { notBridged } from "../internal/not-bridged.ts";

export type * from "@at-inc/pi-ai/utils/kimi-coding";

export const kimiCodingEndpoints: typeof ts.kimiCodingEndpoints = (input) => wasm.aiKimiCodingEndpoints(input);

export const selectKimiCodingEndpoints: typeof ts.selectKimiCodingEndpoints = async () =>
	notBridged("selectKimiCodingEndpoints");

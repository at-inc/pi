// Builds the wasm module into ../wasm: `cargo build` of pi-wasm for wasm32 (release), then the wasm-bindgen CLI with
// `--target web` (ES module glue with an explicit `initSync`, used by both the Node and the Workers entry) and
// `--weak-refs` (handles and closures are freed when JS collects them). The CLI must be the version of the
// wasm-bindgen crate in rust/Cargo.lock; install it with
// `cargo install wasm-bindgen-cli --version <version> --locked`.

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packageDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const rustDir = join(packageDir, "..", "..");
const lock = readFileSync(join(rustDir, "Cargo.lock"), "utf-8");
const version = /\[\[package\]\]\nname = "wasm-bindgen"\nversion = "([^"]+)"/.exec(lock)?.[1];
if (!version) throw new Error("wasm-bindgen is not in rust/Cargo.lock");
const cli = execFileSync("wasm-bindgen", ["--version"], { encoding: "utf-8" }).trim();
if (cli !== `wasm-bindgen ${version}`) {
	throw new Error(
		`${cli} does not match the wasm-bindgen crate ${version}; run: cargo install wasm-bindgen-cli --version ${version} --locked`,
	);
}

execFileSync("cargo", ["build", "--locked", "--target", "wasm32-unknown-unknown", "--release", "-p", "pi-wasm"], {
	cwd: rustDir,
	stdio: "inherit",
});
execFileSync(
	"wasm-bindgen",
	[
		"--target",
		"web",
		"--weak-refs",
		"--out-dir",
		join(packageDir, "wasm"),
		"--out-name",
		"pi_wasm",
		join(rustDir, "target", "wasm32-unknown-unknown", "release", "pi_wasm.wasm"),
	],
	{ stdio: "inherit" },
);

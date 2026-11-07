// The error of a facade method whose Rust side is not bridged yet; each later facade slice removes its call sites.

/** Throws `<name> is not bridged to the Rust core yet`. */
export function notBridged(name: string): never {
	throw new Error(`${name} is not bridged to the Rust core yet`);
}

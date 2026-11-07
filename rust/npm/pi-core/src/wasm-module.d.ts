// .wasm imports as compiled modules (wrangler and workerd module rules).
declare module "*.wasm" {
	const module: WebAssembly.Module;
	export default module;
}

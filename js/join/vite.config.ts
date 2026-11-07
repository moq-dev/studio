import { defineConfig } from "vite";
import { worklet } from "../../moq/js/common/vite-plugin-worklet.ts";

export default defineConfig({
	// Relative asset URLs, so the built page works from any path it is hosted under.
	base: "./",
	// The moq packages are sources, whose audio worklets and video worker this bundles.
	plugins: [worklet()],
	build: {
		target: "esnext",
		// @moq/publish's capture and encode pipeline is most of the page, in one chunk.
		chunkSizeWarningLimit: 1024,
	},
	optimizeDeps: {
		// A prebuilt wasm module, which the dev server's dependency scan cannot bundle.
		exclude: ["@moq/libav"],
	},
});

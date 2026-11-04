import { defineConfig } from "vite";

export default defineConfig({
	// Relative asset URLs, so the built page works from any path it is hosted under.
	base: "./",
	build: {
		target: "esnext",
		// @moq/publish's capture and encode pipeline is most of the page, in one chunk.
		chunkSizeWarningLimit: 1024,
	},
});

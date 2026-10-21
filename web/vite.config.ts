import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// One origin for dev and preview, so dex has one redirect URI and the API one CORS origin.
const server = { host: "localhost", port: 8081, strictPort: true };

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server,
  preview: server,
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});

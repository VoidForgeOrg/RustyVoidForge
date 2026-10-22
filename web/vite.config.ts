import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// One origin for dev and preview, so dex has one redirect URI and the API one CORS origin.
const server = { host: "localhost", port: 8081, strictPort: true };

export default defineConfig({
  plugins: [react(), tailwindcss()],
  server,
  preview: server,
  build: {
    rolldownOptions: {
      output: {
        // HeroUI and React Aria alone pass 500 kB, so the libraries load as their own chunks.
        advancedChunks: {
          groups: [
            { name: "ui", test: /node_modules[\\/](@heroui|@react-aria|@react-stately|react-aria|@kedar42)/ },
            { name: "react", test: /node_modules[\\/](react|react-dom|scheduler)[\\/]/ },
            { name: "vendor", test: /node_modules/ },
          ],
        },
      },
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.{ts,tsx}"],
    setupFiles: ["./src/test-setup.ts"],
    // The contrast test reads the theme's tokens as text.
    css: { include: [/theme\.css/] },
  },
});

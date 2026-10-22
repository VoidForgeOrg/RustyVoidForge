import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { loadEnv, type Plugin } from "vite";
import { defineConfig } from "vitest/config";

// One origin for dev and preview, so dex has one redirect URI and the API one CORS origin.
const server = { host: "localhost", port: 8081, strictPort: true };

/**
 * The built app's Content Security Policy (D-063). Build only: the dev server injects an inline
 * preamble that it would refuse. A meta tag can't carry frame-ancestors.
 */
function csp(env: Record<string, string>): Plugin {
  const api = new URL(env.VITE_API_URL ?? "").origin;
  const issuer = new URL(env.VITE_OIDC_ISSUER ?? "").origin;
  const policy = [
    "default-src 'self'",
    "script-src 'self'",
    "style-src 'self' 'unsafe-inline'",
    "font-src 'self'",
    "img-src 'self' data:",
    `connect-src 'self' ${api} ${issuer}`,
    "base-uri 'none'",
    `form-action ${issuer}`,
  ].join("; ");
  return {
    name: "voidforge-csp",
    apply: "build",
    transformIndexHtml: () => [
      {
        tag: "meta",
        attrs: { "http-equiv": "Content-Security-Policy", content: policy },
        injectTo: "head-prepend",
      },
    ],
  };
}

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss(), csp(loadEnv(mode, import.meta.dirname, "VITE_"))],
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
}));

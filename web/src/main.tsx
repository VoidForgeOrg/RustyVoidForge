import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { restoreLogin } from "./auth/auth";
import { FLOOR_MS } from "./game/refresh";
import { router } from "./router";
import "@fontsource/chakra-petch/latin-500.css";
import "@fontsource/chakra-petch/latin-600.css";
import "./app.css";

// No automatic retries: a failure waits for its Retry-After or the player's Try again (D14).
const queryClient = new QueryClient({
  defaultOptions: { queries: { retry: false, staleTime: FLOOR_MS } },
});

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root");

await restoreLogin();

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);

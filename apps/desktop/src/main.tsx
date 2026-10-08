import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import { TooltipProvider } from "./components/ui/tooltip";
import "./index.css";

// Running `npm run dev` in a plain browser gets a stub backend so the UI can
// be worked on without a Rust rebuild. Dropped entirely from production builds,
// and never active inside Tauri.
if (import.meta.env.DEV) {
  const { installDevMock } = await import("./lib/dev-mock");
  installDevMock();
}

/**
 * Query defaults tuned for a desktop app talking to a local backend:
 * there is no network latency to hide, but a scan is expensive, so nothing
 * refetches on its own. The UI invalidates explicitly when it knows something
 * changed.
 */
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
      retry: false,
    },
  },
});

const container = document.getElementById("root");
if (!container) throw new Error("root element is missing from index.html");

createRoot(container).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <TooltipProvider delay={200}>
        <App />
      </TooltipProvider>
    </QueryClientProvider>
  </StrictMode>,
);

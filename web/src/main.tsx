import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode, lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import { ErrorBoundary } from "./ErrorBoundary";
import { TooltipProvider } from "@/components/ui/tooltip";
import { queryClient } from "@/lib/query-client";
import { makeRouter } from "./router";
import "./index.css";

const root = document.getElementById("root");
if (!root) throw new Error("missing #root");

const router = makeRouter(queryClient);

// Devtools exist only in `vite dev`; the production bundle never contains them.
const Devtools = import.meta.env.DEV
  ? lazy(async () => {
      const [{ ReactQueryDevtools }, { TanStackRouterDevtools }] = await Promise.all([
        import("@tanstack/react-query-devtools"),
        import("@tanstack/react-router-devtools"),
      ]);
      return {
        default: () => (
          <>
            <ReactQueryDevtools buttonPosition="bottom-left" />
            <TanStackRouterDevtools router={router} position="bottom-right" />
          </>
        ),
      };
    })
  : null;

createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider delay={300}>
          <RouterProvider router={router} />
        </TooltipProvider>
        {Devtools ? (
          <Suspense fallback={null}>
            <Devtools />
          </Suspense>
        ) : null}
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
);

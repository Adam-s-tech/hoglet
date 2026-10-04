// The one QueryClient, and the one place a 401 becomes "show the login page".
//
// Defaults: results stay fresh for 30s (endpoints override via queryOptions in
// lib/queries.ts), inactive data is dropped after 5 minutes, 4xx never retries
// (the server said no), anything else retries once. Refetch on window focus is
// off: the freshness poller and live feeds own their own intervals.

import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";
import { ApiError } from "./api";
import { bootKey } from "./queries";

function onApiError(error: unknown): void {
  if (error instanceof ApiError && error.status === 401 && !error.authFlow) {
    queryClient.setQueryData(bootKey, { state: "login" });
  }
}

export const queryClient: QueryClient = new QueryClient({
  queryCache: new QueryCache({ onError: onApiError }),
  mutationCache: new MutationCache({ onError: onApiError }),
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      gcTime: 5 * 60_000,
      refetchOnWindowFocus: false,
      retry: (count, error) => !(error instanceof ApiError && error.status >= 400 && error.status < 500) && count < 1,
    },
  },
});

/** Drop every cached project/user datum (sign-out, sign-in as someone else). Keeps the session query. */
export function clearSessionData(): void {
  queryClient.removeQueries({ predicate: (q) => q.queryKey[0] !== bootKey[0] });
}

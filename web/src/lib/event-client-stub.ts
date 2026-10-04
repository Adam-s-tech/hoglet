// Production-build stand-in for @tanstack/devtools-event-client (see vite.config.ts).
// TanStack Form emits devtools events through it; Hoglet ships no devtools, so
// the real client would only add ~2 KB gzip and a reconnect timer. Same surface
// Form uses: `on` returns an unsubscribe function, `emit` does nothing.

export class EventClient {
  on(): () => void {
    return () => undefined;
  }
  emit(): void {}
}

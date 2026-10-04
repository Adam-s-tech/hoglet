import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

/** Last line of defence: a rendering crash shows a way back, never a blank page. */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Hoglet UI error", error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div role="alert" className="mx-auto mt-[15vh] max-w-lg p-6">
        <h1 className="mb-2 text-xl">This page hit a snag</h1>
        <p className="mb-4 text-muted-foreground">Your data is safe. This is a display problem. Go back, or reload the page.</p>
        <pre className="mb-4 whitespace-pre-wrap text-xs text-muted-foreground">{this.state.error.message}</pre>
        <div className="flex gap-2">
          <Button
            variant="outline"
            onClick={() => {
              window.location.href = "/";
            }}
          >
            Back to home
          </Button>
          <Button onClick={() => window.location.reload()}>Reload</Button>
        </div>
      </div>
    );
  }
}

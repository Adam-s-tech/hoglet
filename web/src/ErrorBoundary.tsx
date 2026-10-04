import { Component, type ErrorInfo, type ReactNode } from "react";

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
      <div style={{ maxWidth: 520, margin: "15vh auto", padding: 24, fontFamily: "inherit" }}>
        <h1 style={{ fontSize: 20, marginBottom: 8 }}>This page hit a snag</h1>
        <p style={{ opacity: 0.75, marginBottom: 16 }}>
          Your data is safe — this is a display problem. Go back, or reload the page.
        </p>
        <pre style={{ fontSize: 12, opacity: 0.6, whiteSpace: "pre-wrap", marginBottom: 16 }}>
          {this.state.error.message}
        </pre>
        <button type="button" onClick={() => { window.location.href = "/"; }}>
          Back to home
        </button>{" "}
        <button type="button" onClick={() => window.location.reload()}>
          Reload
        </button>
      </div>
    );
  }
}

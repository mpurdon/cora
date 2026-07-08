import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CalloutApp } from "./windows/CalloutApp";
import { MainApp } from "./windows/MainApp";
import "./styles.css";

function report(message: string) {
  void invoke("log_frontend_error", { message }).catch(() => {});
}

window.addEventListener("error", (e) =>
  report(`${e.message} @ ${e.filename}:${e.lineno}\n${e.error?.stack ?? ""}`),
);
window.addEventListener("unhandledrejection", (e) =>
  report(`unhandled rejection: ${e.reason?.stack ?? String(e.reason)}`),
);

class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: React.ErrorInfo) {
    report(`render crash: ${error.stack ?? error.message}\n${info.componentStack ?? ""}`);
  }
  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 32, fontFamily: "ui-monospace, monospace", fontSize: 12 }}>
          <h2 style={{ marginBottom: 12 }}>CORA hit a rendering error</h2>
          <pre style={{ whiteSpace: "pre-wrap", color: "#e5534b" }}>
            {this.state.error.stack ?? String(this.state.error)}
          </pre>
        </div>
      );
    }
    return this.props.children;
  }
}

const label = getCurrentWindow().label;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>{label === "callout" ? <CalloutApp /> : <MainApp />}</ErrorBoundary>
  </React.StrictMode>,
);

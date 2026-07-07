import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { CalloutApp } from "./windows/CalloutApp";
import { MainApp } from "./windows/MainApp";
import "./styles.css";

const label = getCurrentWindow().label;

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{label === "callout" ? <CalloutApp /> : <MainApp />}</React.StrictMode>,
);

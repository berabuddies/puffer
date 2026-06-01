import "./app.css";
import { mount } from "svelte";
import App from "./App.svelte";
import MiniWindow from "./lib/mini/MiniWindow.svelte";
import { detectPlatform } from "./lib/shell/platform";

// Tag <html> with a platform class so CSS can adapt chrome without
// reading the userAgent from every component.
const platform = detectPlatform();
if (platform !== "web") {
  document.documentElement.classList.add("is-tauri");
  document.documentElement.classList.add(`is-${platform}`);
}

// The hotkey-summoned mini window loads index.html#mini; render the compact
// launcher there instead of the full app shell.
const isMini = window.location.hash === "#mini";
if (isMini) {
  document.documentElement.classList.add("is-mini");
}

const app = mount(isMini ? MiniWindow : App, {
  target: document.getElementById("app")!
});

export default app;

/**
 * Show raw agent activity (thinking content, tool ids) in dev builds so
 * developers can verify what the model is doing. Prod builds collapse
 * everything unknown to "I'm working on it now..." per product spec.
 *
 * `import.meta.env.DEV` is true under `vite dev` (and Playwright runs
 * via `vite dev`); false under `vite build`. No runtime cost in prod —
 * Vite tree-shakes the dev branch.
 */
export const SHOW_RAW_AGENT_ACTIVITY = import.meta.env.DEV;

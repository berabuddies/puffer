/** dev 显 raw toolId/原始 args;prod 用 toolLabels 友好文案。
 *  Vite 在 build 时把 import.meta.env.DEV tree-shake 成 false。 */
export const SHOW_RAW_AGENT_ACTIVITY = import.meta.env.DEV;

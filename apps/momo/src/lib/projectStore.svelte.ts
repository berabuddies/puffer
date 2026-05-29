/**
 * Project store (Svelte 5 runes).
 *
 * Momo exposes a single fixed project in the rail, resolved by the backend
 * to a fixed cwd under `$MOMO_HOME/projects/default`. The backend creates
 * the directory on first `list_projects` call, so we can hand the absolute
 * cwd straight to `create_session`. Work / Life are gone — every session
 * lives under this one project.
 *
 * Surface:
 *   - `projects` ($state) — list of `{ id, label, cwd }`.
 *   - `loadProjects()` — fetch from the backend; idempotent.
 *   - `getProjectCwd(id)` / `projectIdForCwd(cwd)` — sync lookups; return
 *     undefined / null until `loadProjects()` has resolved.
 *   - `isProjectId(value)` — narrow a string to `ProjectId`.
 */

import * as ws from "./wsClient";
import { pushToast } from "./toast.svelte";

export type ProjectId = "default";

/** The single project every Momo session lives under. */
export const DEFAULT_PROJECT_ID: ProjectId = "default";

export interface Project {
  id: ProjectId;
  label: string;
  cwd: string;
}

interface ProjectDto {
  id: string;
  label: string;
  cwd: string;
}

export const projects = $state<Project[]>([]);

let loadPromise: Promise<void> | null = null;

export function isProjectId(value: string): value is ProjectId {
  return value === "default";
}

export async function loadProjects(): Promise<void> {
  if (loadPromise) return loadPromise;
  loadPromise = (async () => {
    let list: ProjectDto[];
    try {
      list = await ws.request<ProjectDto[]>("list_projects", {});
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      pushToast(`Could not load projects: ${msg}`, "error");
      loadPromise = null;
      throw err;
    }
    const next: Project[] = [];
    for (const p of list) {
      if (!isProjectId(p.id)) continue;
      next.push({ id: p.id, label: p.label, cwd: p.cwd });
    }
    projects.splice(0, projects.length, ...next);
  })();
  return loadPromise;
}

export function getProjectCwd(id: ProjectId): string | undefined {
  return projects.find((p) => p.id === id)?.cwd;
}

export function projectIdForCwd(cwd: string): ProjectId | null {
  const hit = projects.find((p) => p.cwd === cwd);
  return hit ? hit.id : null;
}

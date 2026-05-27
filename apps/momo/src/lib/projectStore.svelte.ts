/**
 * Project store (Svelte 5 runes).
 *
 * Momo currently exposes two fixed projects in the rail: "Work" and "Life".
 * Each project corresponds to a fixed cwd resolved by the backend (under
 * `$MOMO_HOME/projects/<id>`). The backend creates the directories on first
 * `list_projects` call, so we can hand the absolute cwd straight to
 * `create_session`.
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

export type ProjectId = "work" | "life";

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
  return value === "work" || value === "life";
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

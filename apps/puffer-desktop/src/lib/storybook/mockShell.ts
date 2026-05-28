import type { ActiveAgent, UserChip } from "../shell/Sidebar.svelte";
import { AGENTS, PROJECTS, agentPufferState } from "../data/mockProjects";

export const storyAgents: ActiveAgent[] = AGENTS.map((agent, index) => {
  const project = PROJECTS.find((candidate) => candidate.id === agent.project);
  return {
    id: agent.id,
    name: agent.name,
    title: agent.title,
    project: project?.name ?? agent.project,
    projectKey: agent.project,
    branch: agent.branch,
    state: agentPufferState(agent.status),
    updatedAtMs: Date.now() - (index + 1) * 90_000,
    pinned: index < 2
  };
});

export const storyUser: UserChip = {
  initials: "YU",
  name: "Yuna",
  meta: "Local workspace"
};

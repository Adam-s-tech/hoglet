import { createContext, useContext } from "react";
import type { Organization, Project, Workspace } from "./api";

export interface AppState {
  workspace: Workspace;
  project: Project;
  organization: Organization;
  /** Re-fetch the workspace (after creating a project, say). */
  refreshWorkspace: () => Promise<Workspace | null>;
  logout: () => Promise<void>;
}

export const AppContext = createContext<AppState | null>(null);

export function useApp(): AppState {
  const ctx = useContext(AppContext);
  if (!ctx) throw new Error("useApp outside the app shell");
  return ctx;
}

/** Current project id; every project-scoped request uses it. */
export function useProjectId(): string {
  return useApp().project.id;
}

export function projectPath(projectId: string, sub = ""): string {
  const tail = sub.replace(/^\/+/, "");
  return `/project/${encodeURIComponent(projectId)}${tail ? `/${tail}` : ""}`;
}

/** Project-relative link builder for the current project. */
export function usePath(): (sub?: string) => string {
  const id = useProjectId();
  return (sub = "") => projectPath(id, sub);
}

export function canEdit(org: Organization): boolean {
  return org.role === "owner" || org.role === "admin";
}

const LAST_PROJECT = "hoglet.project";
export function rememberProject(id: string): void {
  try {
    localStorage.setItem(LAST_PROJECT, id);
  } catch {
    // ignore
  }
}
export function lastProject(): string | null {
  try {
    return localStorage.getItem(LAST_PROJECT);
  } catch {
    return null;
  }
}

export function findProject(workspace: Workspace, id: string | null): { project: Project; organization: Organization } | null {
  for (const organization of workspace.organizations) {
    for (const project of organization.projects) {
      if (project.id === id) return { project, organization };
    }
  }
  return null;
}

export function firstProject(workspace: Workspace): { project: Project; organization: Organization } | null {
  for (const organization of workspace.organizations) {
    if (organization.projects.length) return { project: organization.projects[0], organization };
  }
  return null;
}

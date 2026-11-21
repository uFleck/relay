import { invoke } from "@tauri-apps/api/core";
import { call, inTauri, onEvent } from "./ipc";
import type { ProviderKey } from "./route";
import type { RepoCandidate } from "./types";
import type { V2Project, V2Settings } from "./ipc";

/* Typed IPC for the v2 Settings surface. The backend only ever returns a source, a
   status, a sanitized reason and a last-four mask; no credential value crosses here. */

export type ProviderState = "missingSetup" | "configuredUnverified" | "verified" | "unavailable";
export type ProviderSource = "relay" | "externalSession" | "connector" | "cli" | "none";

export interface ProviderStatus {
  source: ProviderSource;
  status: ProviderState;
  reason: string | null;
  lastFour: string | null;
}

export type ProviderStatusMap = Partial<Record<ProviderKey, ProviderStatus>>;

export interface T3MappingStatus {
  status: ProviderState;
  reason: string;
}

export interface ImportScope {
  workspaceId: string;
  spaceId: string | null;
  projectId: string;
}

export type T3TargetKind = "id" | "path";
export interface T3TargetInput {
  kind: T3TargetKind;
  value: string;
}

/** Writes must surface errors; a missing command must never look like a saved secret. */
async function write<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  if (!inTauri) throw new Error("This setting can only be changed in the Relay desktop app.");
  return await invoke<T>(cmd, args);
}

export const getProviderStatus = () =>
  call<ProviderStatusMap>("v2_provider_status", undefined, () => ({}));

export const refreshProviders = () =>
  call<ProviderStatusMap>("v2_provider_refresh", undefined, () => ({}));

export const onProviderStatus = (handler: (statuses: ProviderStatusMap) => void) =>
  onEvent("relay://settings-status", handler);

export const onSettingsChange = (handler: () => void) =>
  onEvent("relay://settings-change", handler);

export const setGithubToken = (token: string) =>
  write<ProviderStatus>("v2_github_set_token", { token });

export const clearGithubToken = () =>
  write<ProviderStatus>("v2_github_clear_token", {});

export const setClickupToken = (token: string) =>
  write<ProviderStatus>("v2_clickup_set_token", { token });

export const clearClickupToken = () =>
  write<ProviderStatus>("v2_clickup_clear_token", {});

export const clickupWorkspaces = () =>
  call<{ id: string; name: string }[]>("v2_clickup_workspaces", undefined, () => []);

export const clickupSpaces = (workspaceId: string) =>
  call<{ id: string; name: string }[]>("v2_clickup_spaces", { workspaceId }, () => []);

export const saveV2Settings = (update: {
  importClickupEnabled: boolean;
  importGithubEnabled: boolean;
  importScope: string | null;
}) => write<V2Settings>("v2_settings_update", { update });

export const setOpencodeModel = (model: string) =>
  write<V2Settings>("v2_opencode_set_model", { model });

export const setOpencodeKey = (key: string) =>
  write<ProviderStatus>("v2_opencode_set_key", { key });

export const clearOpencodeKey = () =>
  write<ProviderStatus>("v2_opencode_clear_key", {});

export const scanV2Repos = (folders: string[]) =>
  write<RepoCandidate[]>("v2_repos_scan", { folders });

export const registerV2Repo = (path: string, owner: string, repo: string) =>
  write<V2Project>("v2_repos_register", { path, owner, repo });

export const setT3Mapping = (projectId: string, target: T3TargetInput | null) =>
  write<V2Project>("v2_t3_set_target", { projectId, target });

export const checkT3Mapping = (projectId: string) =>
  write<T3MappingStatus>("v2_t3_check_target", { projectId });

export const errorText = (error: unknown) =>
  error && typeof error === "object" && "message" in error
    ? String((error as { message: unknown }).message)
    : String(error);

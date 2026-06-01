import { apiGet, apiSend } from "@/api/client";

export type SettingType = "number" | "bool" | "enum" | "csv" | "string";
export type SettingValue = string | number | boolean;

export type SettingField = {
  key: string;
  group: string;
  type: SettingType;
  value: SettingValue;
  default: SettingValue;
  range?: { min: number; max: number };
  options?: string[];
  needs_restart: boolean;
  needs_reindex: boolean;
  editable: boolean;
  description: string;
};

export type SettingsResponse = { fields: SettingField[] };

export type SettingsPutResponse = {
  ok: boolean;
  settings?: SettingsResponse;
  needs_restart?: boolean;
  needs_reindex?: boolean;
  errors?: { key: string; message: string }[];
};

export function fetchSettings(signal?: AbortSignal): Promise<SettingsResponse> {
  return apiGet<SettingsResponse>("/settings", signal);
}

export function putSettings(
  changes: Record<string, SettingValue>,
): Promise<SettingsPutResponse> {
  return apiSend<SettingsPutResponse>("PUT", "/settings", { changes });
}

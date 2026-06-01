import type { SettingField, SettingValue } from "@/api/settings";

export type SettingsDraft = Record<string, SettingValue>;

/** Keys present in the draft whose value differs from the server's value. */
export function changedKeys(fields: SettingField[], draft: SettingsDraft): string[] {
  return fields.filter((f) => f.key in draft && draft[f.key] !== f.value).map((f) => f.key);
}

/** The differing key/value pairs, ready to PUT. */
export function pendingChanges(
  fields: SettingField[],
  draft: SettingsDraft,
): Record<string, SettingValue> {
  const out: Record<string, SettingValue> = {};
  for (const f of fields) {
    if (f.key in draft && draft[f.key] !== f.value) {
      out[f.key] = draft[f.key]!;
    }
  }
  return out;
}

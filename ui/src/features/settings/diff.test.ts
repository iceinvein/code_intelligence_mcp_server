import { expect, test } from "bun:test";
import type { SettingField } from "@/api/settings";
import { changedKeys, pendingChanges } from "@/features/settings/diff";

function field(key: string, value: number): SettingField {
  return {
    key,
    group: "Retrieval",
    type: "number",
    value,
    default: value,
    needs_restart: true,
    needs_reindex: false,
    editable: true,
    description: "",
  };
}

test("changedKeys lists only drafts that differ from the server value", () => {
  const fields = [field("a", 0.7), field("b", 0.3)];
  const draft = { a: 0.8, b: 0.3 };
  expect(changedKeys(fields, draft)).toEqual(["a"]);
});

test("pendingChanges returns only the differing key/value pairs", () => {
  const fields = [field("a", 0.7), field("b", 0.3)];
  const draft = { a: 0.8, b: 0.3 };
  expect(pendingChanges(fields, draft)).toEqual({ a: 0.8 });
});

test("a draft equal to the server value is not dirty", () => {
  const fields = [field("a", 0.7)];
  expect(changedKeys(fields, { a: 0.7 })).toEqual([]);
});

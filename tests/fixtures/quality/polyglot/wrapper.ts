import { canonicalQualityTarget } from "./canonical";

export function qualityWrapper(value: number): number {
  return canonicalQualityTarget(value);
}

export function typescriptQualityLeaf(value: number): number {
  return value + 1;
}

export function typescriptQualityAnchor(value: number): number {
  return typescriptQualityLeaf(value);
}

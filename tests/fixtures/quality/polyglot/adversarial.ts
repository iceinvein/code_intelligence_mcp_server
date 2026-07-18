export function qualityOverload(value: string): string;
export function qualityOverload(value: number): number;
export function qualityOverload(value: string | number): string | number {
  return value;
}

export const qualityDecorated = (_target: object, _key: string): void => {};

export class QualityDecoratedService {
  @qualityDecorated
  runQualityTask(): string {
    return "quality-task";
  }
}

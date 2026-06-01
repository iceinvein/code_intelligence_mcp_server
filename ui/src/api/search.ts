import { apiSend } from "@/api/client";

export type SearchHit = {
  id: string;
  name: string;
  kind: string;
  file_path: string;
  score: number;
};

export type SearchResultData = {
  query: string;
  limit: number;
  hits: SearchHit[];
  hits_budget: { returned_count: number; total_count: number; truncated: boolean };
};

export type DefinitionEntry = {
  id: string;
  file_path: string;
  language: string;
  kind: string;
  name: string;
  exported: boolean;
  start_line: number;
  end_line: number;
};

export type DefinitionData = {
  symbol_name: string;
  count: number;
  definitions: DefinitionEntry[];
  context: string;
  disambiguation?: { hint: string; available_files: string[] };
};

export type ReferenceEdge = {
  from_symbol_name: string;
  from_symbol_file: string;
  reference_type: string;
  at_file: string;
  at_line: number;
};

export type ReferencesData = {
  symbol_name: string;
  reference_type: string;
  count: number;
  references: ReferenceEdge[];
  disambiguation?: { hint: string; available_files: string[] };
};

type QueryEnvelope<T> = {
  ok: boolean;
  command: string;
  repo: { path: string; id: string };
  index: { version_unix_s: number | null; fresh: boolean };
  warnings: unknown[];
  result: T;
};

async function queryPost<T>(
  path: string,
  body: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<T> {
  const env = await apiSend<QueryEnvelope<T>>("POST", path, body, signal);
  return env.result;
}

export function searchCode(
  repoPath: string,
  query: string,
  limit = 25,
  signal?: AbortSignal,
): Promise<SearchResultData> {
  return queryPost<SearchResultData>("/query/search", { repo: repoPath, query, limit }, signal);
}

export function getDefinition(
  repoPath: string,
  symbolName: string,
  file?: string,
  signal?: AbortSignal,
): Promise<DefinitionData> {
  return queryPost<DefinitionData>("/query/definition", { repo: repoPath, symbol_name: symbolName, file }, signal);
}

export function findReferences(
  repoPath: string,
  symbolName: string,
  file?: string,
  signal?: AbortSignal,
): Promise<ReferencesData> {
  return queryPost<ReferencesData>("/query/references", { repo: repoPath, symbol_name: symbolName, file }, signal);
}

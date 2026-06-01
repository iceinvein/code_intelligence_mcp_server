import { queryPost } from "@/api/search";

export type IndexedFile = { path: string; symbol_count: number };
export type FilesData = { files: IndexedFile[] };

export type FileSymbol = {
  id: string;
  language: string;
  kind: string;
  name: string;
  exported: boolean;
  start_byte: number;
  end_byte: number;
  start_line: number;
  end_line: number;
};

export type FileSymbolsData = {
  file_path: string;
  count: number;
  symbols: FileSymbol[];
  file_path_normalized?: string;
};

export type UsageExample = {
  reference_type: string;
  from_file_path: string;
  from_symbol_name: string;
  at_file: string;
  at_line: number;
  snippet: string;
};

export type UsageExamplesData = {
  symbol_name: string;
  count: number;
  examples: UsageExample[];
};

export function fetchFiles(repoPath: string, signal?: AbortSignal): Promise<FilesData> {
  return queryPost<FilesData>("/query/files", { repo: repoPath }, signal);
}

export function fetchFileSymbols(
  repoPath: string,
  filePath: string,
  signal?: AbortSignal,
): Promise<FileSymbolsData> {
  return queryPost<FileSymbolsData>(
    "/query/file-symbols",
    { repo: repoPath, file_path: filePath },
    signal,
  );
}

export function fetchUsageExamples(
  repoPath: string,
  symbolName: string,
  signal?: AbortSignal,
): Promise<UsageExamplesData> {
  return queryPost<UsageExamplesData>(
    "/query/usage-examples",
    { repo: repoPath, symbol_name: symbolName },
    signal,
  );
}

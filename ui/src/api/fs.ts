import { apiGet } from "@/api/client";
import type { FsListing } from "@/api/types";

export function listDir(
  path?: string,
  showHidden?: boolean,
  signal?: AbortSignal,
): Promise<FsListing> {
  const params = new URLSearchParams();
  if (path) params.set("path", path);
  if (showHidden) params.set("show_hidden", "true");
  const qs = params.toString();
  return apiGet<FsListing>(`/fs/list${qs ? `?${qs}` : ""}`, signal);
}

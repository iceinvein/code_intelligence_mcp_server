import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { listDir } from "@/api/fs";

export function useFsList(path: string | undefined, showHidden: boolean, enabled: boolean) {
  return useQuery({
    queryKey: ["fs", path ?? "", showHidden],
    queryFn: ({ signal }) => listDir(path, showHidden, signal),
    enabled,
    placeholderData: keepPreviousData,
  });
}

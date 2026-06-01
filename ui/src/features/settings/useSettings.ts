import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { fetchSettings, putSettings, type SettingValue } from "@/api/settings";

export function useSettings() {
  return useQuery({
    queryKey: ["settings"],
    queryFn: ({ signal }) => fetchSettings(signal),
  });
}

export function usePutSettings() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (changes: Record<string, SettingValue>) => putSettings(changes),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["settings"] });
    },
  });
}

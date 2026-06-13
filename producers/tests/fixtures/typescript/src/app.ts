import { makeService } from "./service";

export function renderUser(id: string) {
  const service = makeService();
  return service.load(id);
}

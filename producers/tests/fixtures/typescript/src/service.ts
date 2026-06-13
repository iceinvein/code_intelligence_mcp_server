export class UserService {
  load(id: string) {
    return { id };
  }
}

export function makeService() {
  return new UserService();
}

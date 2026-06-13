class UserService:
    def load(self, user_id):
        return {"id": user_id}

    def render(self, user_id):
        return self.load(user_id)


def make_service():
    return UserService()

class UserService:
    def load(self, user_id):
        return {"id": user_id}


def make_service():
    return UserService()

import pkg.services
from pkg.services import UserService, make_service


def render_user(user_id):
    service = make_service()
    return service.load(user_id)

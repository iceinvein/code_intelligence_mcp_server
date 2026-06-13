import pkg.services
import pkg.services as services
from pkg.services import UserService, make_service


def render_user(user_id):
    service = make_service()
    return service.load(user_id)


def render_alias_user(user_id):
    service = services.make_service()
    return service.load(user_id)


def render_module_user(user_id):
    service = pkg.services.make_service()
    return service.load(user_id)

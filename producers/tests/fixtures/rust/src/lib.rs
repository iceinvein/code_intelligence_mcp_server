pub struct UserService;

impl UserService {
    pub fn load(&self, id: &str) -> String {
        id.to_string()
    }
}

pub fn make_service() -> UserService {
    UserService
}

pub fn render_user(id: &str) -> String {
    let service = make_service();
    service.load(id)
}

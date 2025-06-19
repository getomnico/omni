use ulid::Ulid;

pub fn generate_ulid() -> String {
    Ulid::new().to_string()
}

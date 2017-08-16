
#[derive(Serialize, Deserialize)]
pub struct Config {
    pub mumble: Mumble,
    pub aws: Aws,
}

#[derive(Serialize, Deserialize)]
pub struct Mumble {
    pub server: String,
}

#[derive(Serialize, Deserialize)]
pub struct Aws {
    pub access_key_id: String,
    pub secret_access_key: String,
}

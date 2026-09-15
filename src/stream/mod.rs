pub mod resolver;
pub mod session;
pub mod twitch_rewind;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub url: String,
    pub quality: String,
    pub is_live: bool,
}

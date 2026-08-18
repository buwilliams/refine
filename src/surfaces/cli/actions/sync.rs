use super::*;

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum CliSyncAuthority {
    Live,
    Remote,
}

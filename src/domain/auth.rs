use serde::{Deserialize, Serialize};

/// How a provider is authenticated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    ApiKey,
    #[default]
    Subscription,
}

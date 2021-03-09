//
//
// #[derive(Clone, Debug)]
// pub struct BosCredentials {
//     access: String,
//     secret: String,
//     security_token: Option<String>,
// }
//
// impl BosCredentials {
//     pub fn access(&self) -> &str {
//         &self.access
//     }
//     pub fn secret(&self) -> &str {
//         &self.secret
//     }
//     pub fn security_token(&self) -> &Option<String> {
//         &self.security_token
//     }
//     pub fn to_string(&self) -> String {
//         if let Some(token) = &self.security_token {
//             format!("ak: {}, sk: {}, sessionToken: {}", self.access, self.secret, token)
//         } else {
//             format!("ak: {}, sk: {}", self.access, self.secret)
//         }
//     }
// }
// Modified from https://github.com/mozilla/sccache/blob/master/src/simples3/credential.rs

use async_trait::async_trait;
use chrono::{offset, DateTime, Duration, Utc};
use serde::Deserialize;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, BosError>;

#[derive(Debug, Error)]
pub enum BosError {
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("chrono error: {0}")]
    Chrono(#[from] chrono::ParseError),
}

#[derive(Clone, Debug)]
pub struct BosCredentials {
    access: String,
    secret: String,
    security_token: Option<String>,
}

impl BosCredentials {
    pub fn access(&self) -> &str {
        &self.access
    }
    pub fn secret(&self) -> &str {
        &self.secret
    }
    pub fn security_token(&self) -> &Option<String> {
        &self.security_token
    }
}

/// A trait for types that produce `ObsCredentials`.
#[async_trait]
pub trait ProvideBosCredentials: Send + Sync {
    /// Produce a new `ObsCredentials`.
    async fn credentials(&self) -> Result<BosCredentials>;
}

/// Provides OBS credentials from a resource's IAM role.
pub struct BosIamProvider {
    access: String,
    secret: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl BosIamProvider {
    pub fn new(access: &str, secret: &str, token: Option<&str>) -> Self {
        BosIamProvider {
            access: access.to_owned(),
            secret: secret.to_owned(),
            token: token.map(str::to_string()),
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize, Debug)]
struct OpenStackResponseDe {
    access: String,
    secret: String,
    #[serde(rename = "securitytoken")]
    security_token: String,
    expires_at: String,
}

#[derive(Deserialize, Debug)]
struct OpenStackResponseDeWrapper {
    credential: OpenStackResponseDe,
}

#[async_trait]
impl ProvideBosCredentials for BosIamProvider {
    async fn credentials(&self) -> Result<BosCredentials> {
        // use reqwest::header;
        // use std::str::FromStr;
        //
        // let OpenStackResponseDeWrapper {
        //     credential:
        //     OpenStackResponseDe {
        //         access,
        //         secret,
        //         security_token,
        //         expires_at,
        //     },
        // } = self
        //     .client
        //     .get(BOS_IAM_CREDENTIALS_URL)
        //     .header(header::CONNECTION, "close")
        //     .send()
        //     .await?
        //     .json()
        //     .await?;

        Ok(BosCredentials {
            access: self.access.clone(),
            secret: self.secret.clone(),
            security_token: Some(security_token),
        })
    }
}

use std::cell::RefCell;
use tokio::sync::Mutex;

/// Wrapper for ProvideAwsCredentials that caches the credentials returned by the
/// wrapped provider.  Each time the credentials are accessed, they are checked to see if
/// they have expired, in which case they are retrieved from the wrapped provider again.
pub struct BosAutoRefreshingProvider<P: ProvideBosCredentials> {
    credentials_provider: P,
    cached_credentials: Mutex<RefCell<Option<BosCredentials>>>,
}

impl<P: ProvideBosCredentials> BosAutoRefreshingProvider<P> {
    pub fn new(provider: P) -> BosAutoRefreshingProvider<P> {
        BosAutoRefreshingProvider {
            cached_credentials: Default::default(),
            credentials_provider: provider,
        }
    }
}

#[async_trait]
impl<P: ProvideBosCredentials> ProvideBosCredentials for BosAutoRefreshingProvider<P> {
    async fn credentials(&self) -> Result<BosCredentials> {
        let guard = self.cached_credentials.lock().await;
        let is_invalid = match guard.borrow().as_ref() {
            Some(credentials) => credentials.credentials_are_expired(),
            None => true,
        };
        return if is_invalid {
            match self.credentials_provider.credentials().await {
                Ok(credentials) => {
                    guard.replace(Some(credentials.clone()));
                    Ok(credentials)
                }
                Err(e) => Err(e),
            }
        } else {
            Ok(guard.borrow().as_ref().unwrap().clone())
        };
    }
}

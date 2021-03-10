use thiserror::Error;

pub type Result<T> = std::result::Result<T, BosError>;

#[derive(Debug, Error)]
pub enum BosError {
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("chrono error: {0}")]
    Chrono(#[from] chrono::ParseError),
    #[error("url parse error: {0}")]
    From(#[from] url::ParseError),
}

#[derive(Clone, Debug)]
pub struct BosCredentials {
    access: String,
    secret: String,
    security_token: Option<String>,
}

#[allow(dead_code)]
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

/// Provides OBS credentials from a resource's IAM role.
pub struct BosIamProvider {
    access: String,
    secret: String,
    token: Option<String>,
}

impl BosIamProvider {
    pub fn new(access: &str, secret: &str, token: Option<&str>) -> Self {
        BosIamProvider {
            access: access.to_owned(),
            secret: secret.to_owned(),
            token: token.map(str::to_string),
        }
    }

    pub fn credentials(&self) -> Result<BosCredentials> {
        Ok(BosCredentials {
            access: self.access.clone(),
            secret: self.secret.clone(),
            security_token: self.token.clone(),
        })
    }
}

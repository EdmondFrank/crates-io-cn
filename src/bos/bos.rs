use core::fmt;
use std::str;
use bytes::Bytes;
use reqwest::Response;
use sha2::Sha256;
use hmac::{Hmac, NewMac, Mac};
use super::credentials::*;
use url::Url;
use md5::{Md5, Digest};
use chrono::{Utc, SecondsFormat};
use reqwest::header;

#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
/// Whether or not to use SSL.
pub enum BosSsl {
    /// Use SSL.
    Yes,
    /// Do not use SSL.
    No,
}

fn base_url(endpoint: &str, bucket_name: &str, ssl: BosSsl) -> String {
    format!(
        "{}://{}/{}",
        match ssl {
            BosSsl::Yes => "https",
            BosSsl::No => "http",
        },
        endpoint,
        bucket_name
    )
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    let mut hmac256 = HmacSha256::new_varkey(key).expect("HMAC can take key of any size");
    hmac256.update(data);
    hmac256.finalize().into_bytes().as_slice().to_vec()
}

pub fn md5_hash(content: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(content);
    base64::encode(hasher.finalize())
}

fn signature(string_to_sign: &str, signing_key: &str) -> String {
    let s = hmac(signing_key.as_bytes(), string_to_sign.as_bytes());
    base16::encode_lower(&s)
}

/// An Bos bucket.
pub struct BosBucket {
    name: String,
    base_url: String,
    host: String,
    client: reqwest::Client,
}

impl fmt::Display for BosBucket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bucket(name={}, base_url={})", self.name, self.base_url)
    }
}

impl BosBucket {
    pub fn new(name: &str, endpoint: &str, ssl: BosSsl) -> BosBucket {
        let base_url = base_url(&endpoint, &name, ssl);
        BosBucket {
            name: name.to_owned(),
            base_url,
            host: endpoint.to_owned(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn head(&self, key: &str, creds: &BosCredentials) -> Result<Response> {
        let url = format!("{}/{}", self.base_url, key);
        let request = self.client.head(&url);
        let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let request_url = Url::parse(&url)?;
        let path = request_url.path();
        let content_type = "application/octet-stream";

        let auth = self.auth(
            "HEAD",
            path,
            "",
            0,
            "",
            &date,
            content_type,
            creds,
        );
        let request = request
            .header("Content-Md5", "")
            .header(header::CONTENT_LENGTH, 0)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::HOST, &self.host)
            .header("X-BCE-Date", date.clone())
            .header(header::AUTHORIZATION, auth.clone());

        let result = request.send().await?;
        let headers = result.headers();
        let bce_request_id = headers
            .get("x-bce-request-id")
            .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
        let bce_debug_id = headers
            .get("x-bce-debug-id")
            .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
        trace!(
            "BOS status: [{}] x-bce-request-id: {:?}, x-bce-debug-id: {:?}",
            result.status(),
            bce_request_id,
            bce_debug_id
        );
        Ok(result)
    }

    pub async fn put(&self, key: &str, content: Bytes, creds: &BosCredentials) -> Result<Response> {
        let url = format!("{}/{}", self.base_url, key);
        debug!("PUT {}", url);
        let request = self.client.put(&url);

        let content_type = "application/octet-stream";
        let date = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
        let request_url = Url::parse(&url)?;
        let md5 = md5_hash(&content);
        let content_length = content.len();
        let query_string = request_url.query().unwrap_or("");
        let path = request_url.path();

        let auth = self.auth(
            "PUT",
            path,
            query_string,
            content_length,
            &md5,
            &date,
            content_type,
            creds,
        );
        debug!("auth {}", auth);



        let request = request
            .header("Content-Md5", md5)
            .header(header::CONTENT_LENGTH, content.len())
            .header(header::CONTENT_TYPE, content_type)
            .header(header::HOST, &self.host)
            .header("X-BCE-Date", date.clone())
            .header(header::AUTHORIZATION, auth.clone())
            .body(content.clone());

        let result = request.send().await?;
        let headers = result.headers();
        let bce_request_id = headers
            .get("x-bce-request-id")
            .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
        let bce_debug_id = headers
            .get("x-bce-debug-id")
            .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
        trace!(
            "BOS status: [{}] x-bce-request-id: {:?}, x-bce-debug-id: {:?}",
            result.status(),
            bce_request_id,
            bce_debug_id
        );
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn auth(
        &self,
        verb: &str,
        canonical_uri: &str,
        canonical_query_string: &str,
        content_length: usize,
        md5: &str,
        date: &str,
        content_type: &str,
        creds: &BosCredentials,
    ) -> String {

        let expiration = 1800;
        let sign_key_info = format!(
            "bce-auth-v1/{access}/{date}/{expiration}",
            access = creds.access(),
            date = date,
            expiration = expiration,
        );
        trace!("sign_key_info: {}", sign_key_info);

        let sign_key = signature(&sign_key_info, creds.secret());
        trace!("sign_key: {}", sign_key);

        let canonical_headers = format!(
            "content-length:{content_length}\n\
            content-md5:{md5}\n\
            content-type:{ty}\n\
            host:{host}\n\
            x-bce-date:{date}",
            content_length = content_length,
            md5 = urlencoding::encode(md5),
            ty = urlencoding::encode(content_type),
            host = self.host,
            date = urlencoding::encode(date),
        );

        let canonical_request = format!(
            "{verb}\n{canonical_uri}\n{canonical_query_string}\n{canonical_headers}",
            verb = verb,
            canonical_uri = canonical_uri,
            canonical_query_string = canonical_query_string,
            canonical_headers = canonical_headers,
        );

        let signature = signature(&canonical_request, &sign_key);

        trace!("signature: {}", signature);
        let header_str = "content-length;content-md5;content-type;host;x-bce-date";
        let final_signature = format!("{}/{}/{}", sign_key_info, header_str, signature);
        format!("BOS auth: {}", final_signature);
        final_signature
    }
}

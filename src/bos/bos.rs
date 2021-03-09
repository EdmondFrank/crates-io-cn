// Modified from https://github.com/mozilla/sccache/blob/master/src/simples3/s3.rs
use core::fmt;

use bytes::Bytes;
use reqwest::Response;
use sha2::Sha256;
use hmac::{Hmac, NewMac, Mac};
use super::credentials::*;
// use chrono::{DateTime, NaiveDateTime, Utc};
// use std::time::SystemTime;



#[derive(Debug, Copy, Clone)]
#[allow(dead_code)]
/// Whether or not to use SSL.
pub enum BosSsl {
    /// Use SSL.
    Yes,
    /// Do not use SSL.
    No,
}

fn base_url(endpoint: &str, ssl: BosSsl) -> String {
    format!(
        "{}://{}/",
        match ssl {
            BosSsl::Yes => "https",
            BosSsl::No => "http",
        },
        endpoint
    )
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    type HmacSha256 = Hmac<Sha256>;
    let mut hmac256 = HmacSha256::new_varkey(key).expect("HMAC can take key of any size");
    hmac256.update(data);
    hmac256.finalize().into_bytes().as_slice().to_vec()
}

fn signature(string_to_sign: &str, signing_key: &str) -> String {
    let s = hmac(signing_key.as_bytes(), string_to_sign.as_bytes());
    base64::encode(&s)
}

/// An Obs bucket.
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
        let base_url = base_url(&endpoint, ssl);
        BosBucket {
            name: name.to_owned(),
            base_url,
            host: format!("{}.{}", &name, &endpoint),
            client: reqwest::Client::new(),
        }
    }

    pub async fn put(&self, key: &str, content: Bytes, creds: &BosCredentials) -> Result<Response> {
        use chrono::Utc;
        use reqwest::header;

        let url = format!("{}{}", self.base_url, key);
        debug!("PUT {}", url);
        let request = self.client.put(&url);

        let content_type = "application/octet-stream";
        let date = Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let mut canonical_headers = String::new();
        let request = if let Some(ref token) = creds.security_token() {
            canonical_headers.push_str(format!("{}:{}\n", "x-obs-security-token", token).as_ref());
            request.header("x-obs-security-token", token)
        } else {
            request
        };

        let auth = self.auth(
            "PUT",
            &date,
            key,
            "",
            &canonical_headers,
            content_type,
            creds,
        );
        let request = request
            .header(header::DATE, date)
            .header(header::HOST, &self.host)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, content.len())
            .header(header::AUTHORIZATION, auth)
            .body(content);

        let result = request.send().await?;
        let headers = result.headers();
        let request_id = headers
            .get("x-obs-request-id")
            .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
        let obs_id = headers
            .get("x-obs-id-2")
            .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
        trace!(
            "obs result: [{}], x-obs-request-id: {:?}, x-obs-id-2: {:?}",
            result.status(),
            request_id,
            obs_id
        );
        Ok(result)
    }

    /// https://github.com/baidubce/bce-sdk-go/blob/master/auth/signer.go
    ///
    /// StringToSign definition:
    /// ```text
    /// StringToSign = HTTP-Verb + "\n"
    /// + Content-MD5 + "\n"
    /// + Content-Type + "\n"
    /// + Date + "\n"
    /// + CanonicalizedHeaders + CanonicalizedResource
    /// ```
    ///
    /// CanonicalizedHeaders definition:
    /// 1. filter all header starts with `x-obs-`, convert to lowercase
    /// 2. sort by dictionary order
    /// 3. append with `key:value\n`, concat duplicate key-value with `,` (example:`key:value1,value2\n`)
    ///
    /// Signature definition:
    /// ```text
    /// Signature = Base64( HMAC-SHA1( SecretAccessKeyID, UTF-8-Encoding-Of( StringToSign ) ) )
    /// ```
    ///
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn auth(
        &self,
        verb: &str,
        date: &str,
        path: &str,
        md5: &str,
        headers: &str,
        content_type: &str,
        creds: &BosCredentials,
    ) -> String {

        let string = format!(
            "{verb}\n{md5}\n{ty}\n{date}\n{headers}{resource}",
            verb = verb,
            md5 = md5,
            ty = content_type,
            date = date,
            headers = headers,
            resource = format!("/{}/{}", self.name, path)
        );
        let signature = signature(&string, creds.secret());
        format!("OBS {}:{}", creds.access(), signature)
    }

    // #[allow(clippy::too_many_arguments)]
    // pub(crate) fn sign(
    //     &self,
    //     credentials: &BosCredentials,
    //     http_method: &str,
    //     path: &str,
    //     headers: &str,
    //     params: &str,
    //     timestamp: Option<i64>,
    //     expiration_in_seconds: Option<i64>,
    //     headers_to_sign: Box<[&str]>,
    // ) -> String {
    //
    //     let date = if let Some(time) = timestamp {
    //         let navie = NaiveDateTime::from_timestamp(time, 0);
    //         let datetime: DateTime<Utc> = DateTime::from_utc(navie, Utc);
    //         format!("{:?}", datetime)
    //     } else {
    //         let now: DateTime<Utc> = SystemTime::now().into();
    //         let timestamp = now.timestamp();
    //         let navie = NaiveDateTime::from_timestamp(timestamp, 0);
    //         let datetime: DateTime<Utc> = DateTime::from_utc(navie, Utc);
    //         format!("{:?}", datetime)
    //     };
    //
    //     let expiration = if let Some(expire) = expiration_in_seconds {
    //         expire
    //     } else {
    //         1800
    //     };
    //
    //     let sign_key_info = format!(
    //         "bce-auth-v1/{access}/{date}/{expiration}",
    //         access = credentials.access(),
    //         date = date,
    //         expiration = expiration
    //     );
    //
    //
    //     let sign_key = signature(credentials.secret(), sign_key_info.as_str());
    //     let canonical_uri = if path.len() == 0 {
    //         "/".to_string()
    //     } else {
    //         let mut path = urlencoding::encode(path).replace("%2F", "/").to_string();
    //         if path.starts_with("/") { path } else { path.insert_str(0, "/") }
    //     };
    //
    //     let signature = signature(&string, creds.secret());
    //     format!("OBS {}:{}", creds.access(), signature)
    // }
}

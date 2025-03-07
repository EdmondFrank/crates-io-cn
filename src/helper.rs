use bytes::{Bytes, BytesMut};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::env;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};
use tokio_stream::StreamExt;
use crate::ACTIVE_DOWNLOADS;
use crate::bos::{md5_hash, BosBucket, BosIamProvider, BosSsl};
use crate::error::Error;
use elasticsearch::{
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, Transport, TransportBuilder},
    Elasticsearch, IndexParts,
};
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

#[derive(Serialize, Clone, Debug, Deserialize, Hash, Eq, PartialEq)]
pub struct NewCrateDependency {
    pub optional: bool,
    pub default_features: bool,
    pub name: String,
    pub features: Vec<String>,
    pub req: String,
    pub target: Option<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explicit_name_in_toml: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Hash, Eq, PartialEq)]
pub struct CrateReq {
    #[serde(alias = "crate")]
    name: String,
    #[serde(alias = "vers")]
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deps: Option<Vec<NewCrateDependency>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    features: Option<BTreeMap<String, Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cksum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    yanked: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<String>,
}

lazy_static! {
    static ref PROXY: &'static str = Box::leak(env::var("PROXY").unwrap().into_boxed_str());
    #[derive(Debug)]
    static ref CLIENT: reqwest::Client = if PROXY.is_empty() {
        debug!("initialize without proxy");
        reqwest::Client::new()
    } else {
        debug!("initialize with proxy {:?}", Box::leak(env::var("PROXY").unwrap().into_boxed_str()));
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::http(*PROXY).unwrap())
            .build().unwrap()
    };
}

lazy_static! {
    static ref BOS_BUCKET_NAME: &'static str =
        Box::leak(env::var("BOS_BUCKET_NAME").unwrap().into_boxed_str());
    static ref BOS_ENDPOINT: &'static str =
        Box::leak(env::var("BOS_ENDPOINT").unwrap().into_boxed_str());
    static ref BOS_ACCESS: &'static str =
        Box::leak(env::var("BOS_ACCESS").unwrap().into_boxed_str());
    static ref BOS_SECRET: &'static str =
        Box::leak(env::var("BOS_SECRET").unwrap().into_boxed_str());
    static ref BOS_TOKEN: &'static str = Box::leak(env::var("BOS_TOKEN").unwrap().into_boxed_str());
    static ref BOS_CREDENTIALS: BosIamProvider =
        BosIamProvider::new(&BOS_ACCESS, &BOS_SECRET, Some(&BOS_TOKEN));
    static ref BOS_BUCKET: BosBucket = BosBucket::new(&BOS_BUCKET_NAME, &BOS_ENDPOINT, BosSsl::No);
}

lazy_static! {
    static ref ELASTIC_URL: &'static str =
        Box::leak(env::var("ELASTIC_URL").unwrap().into_boxed_str());
    static ref ELASTIC_USER: &'static str =
        Box::leak(env::var("ELASTIC_USER").unwrap().into_boxed_str());
    static ref ELASTIC_PASS: &'static str =
        Box::leak(env::var("ELASTIC_PASS").unwrap().into_boxed_str());
    static ref CRATES_INDEX: &'static str =
        Box::leak(env::var("CRATES_INDEX").unwrap().into_boxed_str());
}

#[derive(Clone, Debug)]
pub struct Crate {
    name: String,
    version: String,
    content_type: String,
    pub content_length: usize,
    pub buffer: Arc<RwLock<BytesMut>>,
    pub notify: watch::Receiver<usize>,
    ptr: usize,
}

impl Crate {
    pub async fn create(krate_req: CrateReq) -> Result<Arc<Self>, Error> {
        if let Some(krate) = ACTIVE_DOWNLOADS.read().await.get(&krate_req) {
            return Ok(krate.clone());
        }
        let mut guard = ACTIVE_DOWNLOADS.write().await;
        let CrateReq {
            name,
            version,
            deps,
            features,
            cksum,
            yanked,
            links,
        } = krate_req.clone();
        let uri = format!(
            "https://crates.io/api/v1/crates/{name}/{version}/download",
            name = name,
            version = version
        );
        let key = format!("{}/{}", name, version);
        let krate_req_key = krate_req.clone();
        let resp = CLIENT.get(&uri).send().await?;
        if resp.status() != StatusCode::OK {
            return Err(Error::FetchFail);
        }
        let content_length = resp.content_length().ok_or(Error::MissingField)? as usize;
        let (tx, rx) = watch::channel(0);
        let krate = Self {
            name: name.clone(),
            version: version.clone(),
            content_type: resp
                .headers()
                .get("content-type")
                .ok_or(Error::MissingField)?
                .to_str()?
                .to_string(),
            content_length,
            buffer: Arc::new(RwLock::new(BytesMut::with_capacity(
                content_length as usize,
            ))),
            notify: rx,
            ptr: 0,
        };
        let write_buffer = krate.buffer.clone();
        tokio::spawn(async move {
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(data) => {
                        let mut buffer = write_buffer.write().await;
                        trace!("recv {}", data.len());
                        buffer.extend_from_slice(&data[..]);
                        tx.send(data.len()).unwrap();
                    }
                    Err(e) => {
                        error!("{}", e);
                        break;
                    }
                };
            }

            let buffer = write_buffer.read().await.clone().freeze();
            debug!("{:?} download complete", krate_req_key);

            let mut skip = false;

            if let Ok(credentials) = BOS_CREDENTIALS.credentials() {
                if let Ok(result) = BOS_BUCKET
                    .head(key.clone().as_str(), &credentials)
                    .await {
                        if result.status().as_u16() != 404 {
                            let content_md5 = result
                                .headers()
                                .get("content-md5")
                                .map_or_else(|| "null", |h| h.to_str().unwrap_or("invalid"));
                            debug!("head result {:?}", result.status());
                            if content_md5 == md5_hash(&buffer) {
                                debug!("crate md5 {:?}", md5_hash(&buffer));
                                debug!("head object meta {:?} ", result.headers());
                                skip = true;
                            }
                        } else {
                            debug!("head result {:?}", result.status());
                        }
                    }
            } else {
                debug!("failed to fetch credentials")
            }

            let mut counter: i32 = 10;
            debug!("begin upload: {}", counter);
            while counter > 0 && !skip {
                debug!("current attempt rest: {}", counter);
                let result = match BOS_CREDENTIALS.credentials() {
                    Ok(credentials) => BOS_BUCKET
                        .put(&key, buffer.clone(), &credentials)
                        .await
                        .err(),
                    Err(e) => Some(e),
                };

                if let Some(e) = result {
                    debug!("retry attempt {}:{:?}", 10 - counter, e);
                    counter -= 1;
                    continue;
                }

                let es_result = Elasticsearch::new(
                    if let (Ok(user), Ok(pass)) = (ELASTIC_USER.parse(), ELASTIC_PASS.parse()) {
                        let url = ELASTIC_URL.to_string();
                        let credentials = Credentials::Basic(user, pass);
                        let conn_pool = SingleNodeConnectionPool::new(Url::parse(&url).unwrap());
                        TransportBuilder::new(conn_pool)
                            .auth(credentials)
                            .build()
                            .unwrap()
                    } else {
                        Transport::single_node(&ELASTIC_URL).unwrap()
                    },
                )
                    .index(IndexParts::IndexId(&CRATES_INDEX, &key))
                    .body(json!(
                        {
                            "name": name,
                            "vers": version,
                            "deps": deps,
                            // "features": format!("{{\"data\": \"{:?}\"}}", features.as_ref().unwrap()),
                            "cksum": cksum,
                            "yanked": yanked,
                            "links": links,
                        }
                    ))
                    .send()
                    .await;

                match es_result {
                    Ok(res) => {
                        debug!(
                            "{}",
                            format!("{{\"data\": \"{:?}\"}}", features.as_ref().unwrap())
                        );
                        debug!(
                            "status: [{}] update document of {} {}",
                            res.status_code(),
                            key,
                            res.text().await.unwrap()
                        );
                    }
                    Err(e) => {
                        error!("failed to update document of {}, error: {}", key, e);
                    }
                }
                ACTIVE_DOWNLOADS.write().await.remove(&krate_req_key);
                debug!("remove {:?} from active download", krate_req_key);
                break;
            }
        });
        guard.insert(krate_req.clone(), Arc::new(krate));
        debug!("insert {:?} into active download", krate_req);
        Ok(guard.get(&krate_req).unwrap().clone())
    }

    pub fn tee(&self, tx: mpsc::UnboundedSender<Result<Bytes, ()>>) {
        let mut notify = self.notify.clone();
        let krate = self.clone();
        tokio::spawn(async move {
            let mut ptr = 0;
            loop {
                let data = {
                    let buffer = krate.buffer.read().await;
                    let data = Bytes::copy_from_slice(&buffer[ptr..]);
                    ptr += data.len();
                    data
                };
                match tx.send(Ok(data)) {
                    Ok(_) => (),
                    Err(e) => {
                        error!("{}", e);
                        break;
                    }
                }
                info!("{}/{}", ptr, krate.content_length);
                if ptr == krate.content_length {
                    break;
                }
                if let Err(e) = notify.changed().await {
                    debug!("{}", e);
                    tx.closed().await;
                    break;
                }
            }
        });
    }
}

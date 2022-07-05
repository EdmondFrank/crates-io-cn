#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

use actix_web::middleware::Logger;
use actix_web::{get, web, App, Error, HttpResponse, HttpServer};
use std::collections::HashMap;
use std::{cmp, env};

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc::unbounded_channel, RwLock};

mod bos;
mod error;
mod helper;
#[allow(dead_code)]
mod index;
use clap::{App as CliApp, Arg};
use elasticsearch::{
    auth::Credentials,
    http::transport::{SingleNodeConnectionPool, Transport, TransportBuilder},
    Elasticsearch, SearchParts,
};
use rev_lines::RevLines;
mod easy_git;
#[cfg(all(feature = "systemd-integration", target_os = "linux"))]
mod systemd;
// mod test;

use crate::index::{Config, GitIndex};
use chrono::{SecondsFormat, Utc};
use helper::{Crate, CrateReq};
use serde_json::Value;
use std::fs::File;
use std::io::BufReader;
use std::ops::{Add, Deref};
use tokio::time::{Duration, Instant};
use url::Url;
use walkdir::{DirEntry, WalkDir};

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

lazy_static! {
    static ref GIT_INDEX_DIR: &'static str =
        Box::leak(env::var("GIT_INDEX_DIR").unwrap().into_boxed_str());
    static ref DL_FORMAT: &'static str = Box::leak(env::var("DL_FORMAT").unwrap().into_boxed_str());
    static ref API_FORMAT: &'static str =
        Box::leak(env::var("API_FORMAT").unwrap().into_boxed_str());
    static ref ACTIVE_DOWNLOADS: Arc<RwLock<HashMap<CrateReq, Arc<Crate>>>> =
        Arc::new(RwLock::new(HashMap::new()));
}

///
/// With this as config in crates.io-index
/// ```json
/// {
///     "dl": "https://bucket-cdn/{crate}/{version}",
///     "api": "https://crates.io"
/// }
/// ```
/// Upyun will redirect 404 (non-exist) crate to given address configured
/// replace `$_URI` with the path part `/{crate}/{version}`
#[get("/sync/{crate}/{version}")]
async fn sync(krate_req: web::Path<CrateReq>) -> HttpResponse {
    let krate_req = krate_req.into_inner();
    format!("{:?}", krate_req);
    match Crate::create(krate_req).await {
        Err(e) => {
            error!("{:?}", e);
            HttpResponse::NotFound().finish()
        }
        Ok(krate) => {
            let (tx, rx) = unbounded_channel::<Result<bytes::Bytes, ()>>();
            let rx = tokio_stream::wrappers::UnboundedReceiverStream::new(rx);
            krate.tee(tx);
            HttpResponse::Ok()
                .content_type("application/x-tar")
                .streaming(rx)
        }
    }
}

#[derive(Deserialize, Serialize)]
struct TotalCrates {
    total: u64,
}

#[derive(Deserialize, Serialize)]
struct Crates {
    crates: Vec<CratePackage>,
    meta: TotalCrates,
}

#[derive(Deserialize, Serialize)]
struct CratesList {
    crates: Vec<CrateItem>,
    meta: PageResults,
}

#[derive(Deserialize, Serialize)]
pub struct CrateItem {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Deserialize, Serialize)]
pub struct CratePackage {
    pub name: String,
    pub description: Option<String>,
    pub max_version: String,
}

#[derive(Deserialize, Debug)]
struct SearchParams {
    q: String,
    version: Option<String>,
    per_page: Option<u32>,
    page: Option<u32>,
}

#[derive(Deserialize, Debug)]
struct ListParams {
    page: Option<u32>,
    per_page: Option<u32>,
}

#[derive(Deserialize, Serialize)]
struct PageResults {
    total: u64,
    current_page: u32,
    per_page: u32,
}

#[get("/sync_status")]
async fn sync_status() -> Result<HttpResponse, Error> {
    // this status interface is compatible with nexus interfaces format [no meaning]
    Ok(HttpResponse::Ok()
        .content_type("application/json")
        .body(json!(
        {
            "id" : "carte-index-sync",
            "name" : "Sync",
            "type" : "script",
            "message" : "Execute script",
            "currentState" : "WAITING",
            "lastRunResult" : "INTERRUPTED",
            "nextRun" : null,
            "lastRun" : Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false)
        })))
}

#[get("/api/v1/list")]
async fn list(params: web::Query<ListParams>) -> Result<HttpResponse, Error> {
    let mut crates = Vec::new();
    let current_page = cmp::max(params.page.unwrap_or(1), 1);
    let per_page = cmp::max(params.per_page.unwrap_or(10), 1);
    let response = Elasticsearch::new(
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
    .search(SearchParts::Index(&[&CRATES_INDEX]))
    .body(json!({
        "query": {
            "match_all": {}
        },
        "size": per_page,
        "from": (current_page - 1) * per_page
    }))
    .send()
    .await
    .expect("failed to connect elastic");
    let response_body = response
        .json::<Value>()
        .await
        .expect("failed to parse elastic result");
    let total = response_body["hits"]["total"]["value"]
        .as_u64()
        .unwrap_or(0);
    for hit in response_body["hits"]["hits"].as_array().unwrap() {
        let name = hit["_source"]["name"].to_string().replace("\"", "");
        let version = hit["_source"]["vers"].to_string().replace("\"", "");
        crates.push(CrateItem {
            id: format!("{}/{}", name, version),
            name,
            version,
        });
    }

    Ok(HttpResponse::Ok().json(&CratesList {
        crates,
        meta: PageResults {
            total,
            current_page,
            per_page,
        },
    }))
}

#[get("/api/v1/crates")]
async fn search(params: web::Query<SearchParams>) -> Result<HttpResponse, Error> {
    format!("Welcome {:?}!", params);
    let mut crates = Vec::new();
    let current_page = cmp::max(params.page.unwrap_or(1), 1);
    let per_page = cmp::max(params.per_page.unwrap_or(10), 1);
    let version = params.version.as_deref().unwrap_or("").trim();
    let query = match version {
        "" => {
            json!({
                "query": {
                    "match": {
                        "name": params.q,
                    }
                },
                "size": per_page,
                "from": (current_page - 1) * per_page,
            })
        }
        _ => {
            json!({
                "query": {
                    "bool": {
                        "must": [
                            {
                                "match": {
                                    "name": params.q
                                }
                            },
                            {
                                "match": {
                                    "vers": version
                                }
                            }
                        ]
                    }
                },
                "size": per_page,
                "from": (current_page - 1) * per_page,
            })
        }
    };
    let response = Elasticsearch::new(
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
    .search(SearchParts::Index(&[&CRATES_INDEX]))
    .body(query)
    .send()
    .await
    .expect("failed to connect elastic");
    let response_body = response
        .json::<Value>()
        .await
        .expect("failed to parse elastic result");
    let total = response_body["hits"]["total"]["value"].as_u64().unwrap();
    for hit in response_body["hits"]["hits"].as_array().unwrap() {
        crates.push(CratePackage {
            name: hit["_source"]["name"].to_string().replace("\"", ""),
            description: Option::from(hit["_id"].to_string().replace("\"", "")),
            max_version: hit["_source"]["vers"].to_string().replace("\"", ""),
        });
    }
    Ok(HttpResponse::Ok().json(&Crates {
        crates,
        meta: TotalCrates { total },
    }))
}

fn is_not_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| entry.depth() == 0 || !s.starts_with("."))
        .unwrap_or(false)
}

async fn sync_by_each_line(entry: &DirEntry) -> Result<(), std::io::Error> {
    let input = File::open(entry.path())?;
    debug!("begin to sync file: {}", entry.path().display());
    let buffered = BufReader::new(input);
    let ddl = Instant::now().add(Duration::from_secs(5));
    let (tx, rx) = async_channel::unbounded();
    for i in 0..10 {
        let worker_rx = rx.clone();
        tokio::spawn(async move {
            while let Ok(krate) = worker_rx.recv().await {
                debug!("[worker#{}]start to sync {:?}", i, krate);
                match Crate::create(krate).await {
                    Ok(_) => (),
                    Err(e) => error!("{:?}", e),
                };
            }
        });
    }
    let crates: Vec<CrateReq> = RevLines::new(buffered)
        .unwrap()
        .filter_map(|l| serde_json::from_str(l.as_str()).ok())
        .collect();
    for krate in crates {
        if let Err(e) = tx.send(krate).await {
            error!("{}", e);
        }
    }

    while !rx.is_empty() || !tx.is_empty() {
        tokio::time::sleep_until(ddl).await;
    }

    println!("sender: {}, receiver: {}", tx.is_empty(), rx.is_empty());

    Ok(())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    log4rs::init_file("config/log4rs.yml", Default::default()).unwrap();
    dotenv::dotenv().ok();
    let matches = CliApp::new("crates.io sync")
        .arg(
            Arg::with_name("sync")
                .short("s")
                .long("sync-all")
                .takes_value(false)
                .help("sync all crate by index"),
        )
        .get_matches();
    let (tx, rx) = async_channel::unbounded();
    for i in 0..10 {
        let worker_rx = rx.clone();
        tokio::spawn(async move {
            while let Ok(krate) = worker_rx.recv().await {
                debug!("[worker#{}]start to sync {:?}", i, krate);
                match Crate::create(krate).await {
                    Ok(_) => (),
                    Err(e) => error!("{:?}", e),
                };
            }
        });
    }
    tokio::spawn(async move {
        let gi = GitIndex::new(
            GIT_INDEX_DIR.deref(),
            &Config {
                dl: DL_FORMAT.to_string(),
                api: API_FORMAT.to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        loop {
            let ddl = Instant::now().add(Duration::from_secs(300));
            info!("next update will on {:?}, exec git update now", ddl);
            #[cfg(all(feature = "systemd", target_os = "linux"))]
            systemd::notify_watchdog();
            let updated = gi.update();
            let crates = match updated {
                Ok(crates) => crates,
                Err(e) => {
                    error!("git update error: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };
            for krate in crates {
                if let Err(e) = tx.send(krate).await {
                    error!("{}", e);
                }
            }
            tokio::time::sleep_until(ddl).await;
        }
    });
    if matches.is_present("sync") {
        debug!("starting sync all crate.io's crates by index");
        tokio::spawn(async move {
            for x in WalkDir::new(GIT_INDEX_DIR.to_string())
                .sort_by(|a_entry, b_entry| {
                    b_entry
                        .metadata()
                        .ok()
                        .unwrap()
                        .modified()
                        .ok()
                        .unwrap()
                        .cmp(&a_entry.metadata().ok().unwrap().modified().ok().unwrap())
                })
                .into_iter()
                .filter_entry(|e| is_not_hidden(e))
                .filter_map(|v| v.ok())
                .filter(|entry| entry.metadata().ok().unwrap().is_file())
            {
                sync_by_each_line(&x)
                    .await
                    .expect(format!("failed to sync {}", x.path().display()).as_str())
            }
        });
    }
    let server_url = env::var("SERVER_URL").expect("SERVER_URL is not set in .env file");
    let server = HttpServer::new(|| {
        App::new()
            .wrap(Logger::default())
            .service(sync)
            .service(search)
            .service(list)
            .service(sync_status)
    })
    .bind(server_url)?
    .run();
    #[cfg(all(feature = "systemd", target_os = "linux"))]
    systemd::notify_ready();
    server.await
}

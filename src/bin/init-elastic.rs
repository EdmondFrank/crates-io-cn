use std::{env};
use url::Url;
use serde_json::{json};
use elasticsearch::{Elasticsearch, auth::Credentials};
use elasticsearch::indices::{IndicesCreateParts, IndicesExistsParts};
use elasticsearch::http::transport::{TransportBuilder, SingleNodeConnectionPool, Transport};

#[macro_use]
extern crate lazy_static;

lazy_static! {
    static ref ELASTIC_URL: &'static str = Box::leak(env::var("ELASTIC_URL").unwrap().into_boxed_str());
    static ref ELASTIC_USER: &'static str = Box::leak(env::var("ELASTIC_USER").unwrap().into_boxed_str());
    static ref ELASTIC_PASS: &'static str = Box::leak(env::var("ELASTIC_PASS").unwrap().into_boxed_str());
    static ref CRATES_INDEX: &'static str = Box::leak(env::var("CRATES_INDEX").unwrap().into_boxed_str());
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::dotenv().ok();
    let transport =  if let (Ok(user), Ok(pass)) = (ELASTIC_USER.parse(), ELASTIC_PASS.parse()) {
        let credentials = Credentials::Basic(user, pass);
        let conn_pool = SingleNodeConnectionPool::new(Url::parse(&ELASTIC_URL)?);
        TransportBuilder::new(conn_pool).auth(credentials).build()?
    } else {
        Transport::single_node(&ELASTIC_URL)?
    };

    let index_name = CRATES_INDEX.to_string();
    let client = Elasticsearch::new(transport);
    let check_result = client
        .indices()
        .exists(IndicesExistsParts::Index(&[&index_name]))
        .send()
        .await?;
    match check_result.status_code().as_u16() {
        404 => {
            let response = client
                .indices()
                .create(IndicesCreateParts::Index(&index_name))
                .body(json!({
                    "mappings": {
                        "properties": {
                            "name": {
                                "type": "text"
                            },
                            "vers": {
                                "type": "text"
                            },
                            "deps": {
                                "properties": {
                                    "optional": {
                                        "type": "boolean"
                                    },
                                    "default_features": {
                                        "type": "boolean"
                                    },
                                    "name": {
                                        "type": "text",
                                    },
                                    "feature": {
                                        "type": "nested",
                                    },
                                    "req": {
                                        "type": "text",
                                    },
                                    "target": {
                                        "type": "text",
                                    },
                                    "kind": {
                                        "type": "text",
                                    }
                                }
                            },
                            "cksum": {
                                "type": "keyword"
                            },
                            "features": {
                                "type": "object"
                            },
                            "yanked": {
                                "type": "boolean"
                            },
                            "links": {
                                "type": "keyword"
                            }
                        }
                    }
                }))
                .send()
                .await?;
            println!("response：{:?}", response.text().await?);
        },
        200 => {
            println!("indices: {:?} exists！", index_name);
        }
        _ => {
            println!("status_code: [{}], response: {:?}！", check_result.status_code(), check_result.text().await?);
        }
    }
    Ok(())
}

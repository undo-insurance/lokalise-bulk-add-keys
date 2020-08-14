use anyhow::{Error, Result};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::{collections::HashSet, env, path::PathBuf};
use structopt::StructOpt;
use tokio::fs;

#[derive(Debug, StructOpt)]
struct Opt {
    /// Name of the project in Lokalise
    #[structopt(short = "p", long = "project")]
    project: String,

    /// Don't upload things to Lokalise, just parse the input file
    #[structopt(long = "dry-run")]
    dry_run: bool,

    /// Input file containing the keys you want to add
    #[structopt(name = "FILE", parse(from_os_str))]
    input: PathBuf,
}

#[tokio::main]
async fn main() {
    match try_main().await {
        Ok(()) => {}
        Err(err) => {
            eprintln!("{}", err);
            std::process::exit(1);
        }
    }
}

async fn try_main() -> Result<()> {
    let opt = Opt::from_args();

    let file_contents = fs::read_to_string(&opt.input).await?;
    let keys_to_add = serde_yaml::from_str::<Data>(&file_contents)?.keys;

    if opt.dry_run {
        println!("{:#?}", keys_to_add);
        return Ok(());
    }

    let lokalise_token = env::var("LOKALISE_API_TOKEN")
        .map_err(|_| Error::msg("Missing env var LOKALISE_API_TOKEN"))?;

    let client = LokaliseClient::new(lokalise_token)?;

    let projects = client.projects().await?;
    let project = projects
        .into_iter()
        .find(|p| p.name == opt.project)
        .ok_or_else(|| Error::msg(format!("No project name '{}' was found", opt.project)))?;

    let all_keys = client.all_keys(&project).await?;
    for key in &keys_to_add {
        if all_keys.contains(&key.key) {
            return Err(Error::msg(format!("The key `{}` already exists", key.key)));
        }
    }

    client.create_keys(&project, keys_to_add).await?;

    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct Data {
    keys: Vec<KeyToAdd>,
}

#[derive(Debug, Deserialize, Serialize)]
struct KeyToAdd {
    key: String,
    #[serde(flatten)]
    translation: Translation,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
enum Translation {
    #[serde(rename = "translation")]
    Singular(String),
    #[serde(rename = "translations")]
    Plural { singular: String, plural: String },
}

#[derive(Debug)]
struct LokaliseClient {
    client: Client,
}

impl LokaliseClient {
    fn new(token: String) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("x-api-token", HeaderValue::from_str(&token)?);
        let client = Client::builder().default_headers(headers).build()?;

        Ok(Self { client })
    }

    async fn projects(&self) -> Result<Vec<Project>> {
        #[derive(Debug, Deserialize)]
        struct ProjectsResponse {
            projects: Vec<Project>,
        }

        let res = self.client.get(&self.url("/projects")).send().await?;

        Ok(res.json::<ProjectsResponse>().await?.projects)
    }

    async fn all_keys(&self, project: &Project) -> Result<HashSet<String>> {
        let mut key_names = HashSet::new();
        let limit = 1000;
        let mut page = 1;

        loop {
            let res = self
                .client
                .get(&self.url(&format!("/projects/{}/keys", &project.id)))
                .query(&[("limit", limit), ("page", page)])
                .send()
                .await?;
            let keys = res.json::<KeysResponse>().await?.keys;

            let keys_count = keys.len();

            for key in keys {
                let key = key.key_name;
                if key.ios == key.android && key.android == key.web && key.web == key.other {
                    key_names.insert(key.ios.clone());
                } else {
                    return Err(Error::msg(
                        "Key with different names per platform isn't supported",
                    ));
                }
            }

            if keys_count < limit {
                break;
            }

            page += 1;
        }

        Ok(key_names)
    }

    async fn create_keys(&self, project: &Project, keys_to_create: Vec<KeyToAdd>) -> Result<()> {
        let payload = json!({
            "keys": keys_to_create.iter().map(|key| {
                let translation = match &key.translation {
                    Translation::Singular(text) => json!({
                        "language_iso": &project.base_language_iso,
                        "translation": text,
                    }),
                    Translation::Plural { singular, plural } => json!({
                        "language_iso": &project.base_language_iso,
                        "translation": {
                            "one": singular,
                            "other": plural,
                        }
                    })
                };

                let is_plural = match &key.translation {
                    Translation::Singular(_) => false,
                    Translation::Plural { .. } => true
                };

                json!({
                    "key_name": &key.key,
                    "translations": [translation],
                    "is_plural": is_plural,
                    "platforms": ["ios", "android", "web", "other"],
                    "tags": &key.tags,
                })
            }).collect::<Vec<_>>()
        });

        let resp = self
            .client
            .post(&self.url(&format!("/projects/{}/keys", &project.id)))
            .json(&payload)
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        let resp_as_keys = serde_json::from_value::<KeysResponse>(resp.clone());
        let resp_as_error = serde_json::from_value::<ErrorResponse>(resp.clone());
        let keys = match (resp_as_keys, resp_as_error) {
            (Ok(keys_resp), Err(_)) => keys_resp.keys,
            (Err(_), Ok(ErrorResponse { error })) => {
                use std::fmt::Write;

                let mut msg = String::new();
                writeln!(msg, "Lokalise request failed").unwrap();

                if error.message == "Unauthorized" {
                    write!(msg, "Got 401 unauthorized. Please ensure your auth token is correct and has both read and write permissions").unwrap();
                } else {
                    write!(msg, "Got {} {}", error.code, error.message).unwrap();
                }

                return Err(Error::msg(msg));
            }
            (Ok(_), Ok(_)) => {
                return Err(Error::msg("Lokalise request both failed and succeeded...?"))
            }
            (Err(_), Err(_)) => return Err(Error::msg("Failed to parse lokalise response")),
        };

        let created_keys = keys
            .into_iter()
            .map(|key| key.key_name.ios)
            .collect::<HashSet<_>>();

        let mut keys_created = vec![];
        let mut keys_not_created = vec![];
        for key in &keys_to_create {
            if created_keys.contains(&key.key) {
                keys_created.push(&key.key);
            } else {
                keys_not_created.push(&key.key);
            }
        }

        if keys_created.is_empty() && keys_not_created.is_empty() {
            println!("No keys to create to seems 👀");
            Ok(())
        } else {
            for key in keys_created {
                println!("✅ {}", key)
            }

            if !keys_not_created.is_empty() {
                for key in keys_not_created {
                    println!("❌ {}", key)
                }

                Err(Error::msg("Failed to create some keys"))
            } else {
                Ok(())
            }
        }
    }

    fn url(&self, url: &str) -> String {
        format!("https://api.lokalise.com/api2{}", url)
    }
}

#[derive(Debug, Deserialize)]
struct Project {
    #[serde(rename = "project_id")]
    id: String,
    name: String,
    base_language_iso: String,
}

#[derive(Debug, Deserialize)]
struct KeysResponse {
    keys: Vec<KeyResponse>,
}

#[derive(Debug, Deserialize)]
struct KeyResponse {
    key_name: KeyName,
}

#[derive(Debug, Deserialize)]
struct KeyName {
    ios: String,
    android: String,
    web: String,
    other: String,
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorResponseInner,
}

#[derive(Debug, Deserialize)]
struct ErrorResponseInner {
    code: u32,
    message: String,
}

use anyhow::{Error, Result};
use reqwest::{
    header::{HeaderMap, HeaderValue},
    Client,
};
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use std::{env, path::PathBuf};
use structopt::StructOpt;
use tokio::fs;

#[derive(Debug, StructOpt)]
struct Opt {
    /// Name of the project in Lokalise
    #[structopt(short = "p", long = "project")]
    project: String,

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

    let lokalise_token = env::var("LOKALISE_API_TOKEN")
        .map_err(|_| Error::msg("Missing env var LOKALISE_API_TOKEN"))?;

    let client = LokaliseClient::new(lokalise_token)?;

    let projects = client.projects().await?;
    let project = projects
        .into_iter()
        .find(|p| p.name == opt.project)
        .ok_or_else(|| Error::msg(format!("No project name '{}' was found", opt.project)))?;

    client.create_keys(&project, keys_to_add).await?;

    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct Data {
    keys: Vec<Key>,
}

#[derive(Debug, Deserialize, Serialize)]
struct Key {
    key: String,
    #[serde(flatten)]
    translation: Translation,
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

    async fn create_keys(&self, project: &Project, keys: Vec<Key>) -> Result<()> {
        let payload = json!({
            "keys": keys.iter().map(|key| {
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
                })
            }).collect::<Vec<_>>()
        });

        self.client
            .post(&self.url(&format!("/projects/{}/keys", &project.id)))
            .json(&payload)
            .send()
            .await?;

        Ok(())
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

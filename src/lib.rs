use jiff::Timestamp;
use jiff::Zoned;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use rig::agent::Agent;
use rig::client::Nothing;
use rig::prelude::*;
use rig::{completion::Prompt, providers::ollama};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create ollama client: {0}")]
    OllamaClient(#[from] rig::http_client::Error),
    #[error("failed to prompt agent: {0}")]
    Prompt(#[from] rig::completion::PromptError),
    #[error("failed to parse LLM response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("failed to parse datetime: {0}")]
    Jiff(#[from] jiff::Error),
    #[error("{0}")]
    Llm(#[from] LlmError),
}

#[derive(Debug, thiserror::Error, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmError {
    #[error("could not parse the input")]
    CouldNotParse,
    #[error("ambiguous timezone abbreviation")]
    AmbiguousTimezone,
    #[error("missing source time")]
    MissingSourceTime,
    #[error("missing source timezone")]
    MissingSourceTimezone,
    #[error("missing target timezones")]
    MissingTargetTimezones,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LlmResponse {
    Ok {
        datetime: String,
        source_tz: String,
        target_tzs: Vec<String>,
    },
    Error {
        error: LlmError,
    },
}

pub struct Conversion {
    pub source: Zoned,
    pub source_tz: String,
    pub targets: Vec<(Zoned, String)>,
}

pub struct TimeAgent {
    agent: Agent<ollama::CompletionModel>,
}

impl TimeAgent {
    pub fn new(base_url: &str, model: &str) -> Result<Self, Error> {
        let client = ollama::Client::builder()
            .api_key(Nothing)
            .base_url(base_url)
            .build()?;
        let agent = client
            .agent(model)
            .preamble(include_str!("./system.txt"))
            .build();
        Ok(Self { agent })
    }

    pub async fn convert(&self, input: &str) -> Result<Conversion, Error> {
        let now = Timestamp::now().to_zoned(TimeZone::UTC).to_string();

        let prompt = include_str!("./prompt.txt")
            .to_owned()
            .replace("[[TIME]]", &now)
            .replace("[[INPUT]]", input);

        let response = self.agent.prompt(prompt).await?;
        let output: LlmResponse = serde_json::from_str(&response)?;

        match output {
            LlmResponse::Ok {
                datetime,
                source_tz,
                target_tzs,
            } => {
                let dt: DateTime = datetime.parse()?;
                let source = dt.in_tz(&source_tz)?;

                let mut targets = Vec::with_capacity(target_tzs.len());
                for tz in target_tzs {
                    let target = source.in_tz(&tz)?;
                    targets.push((target, tz));
                }

                Ok(Conversion {
                    source,
                    source_tz,
                    targets,
                })
            }
            LlmResponse::Error { error } => Err(error.into()),
        }
    }
}

pub fn load_env_file() {
    match dotenvy::dotenv() {
        Ok(path) => println!("Loaded .env from {:?}", path),
        Err(_) => {}
    }
}

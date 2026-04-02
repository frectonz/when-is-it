use std::sync::Arc;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use clap::Parser;
use color_eyre::eyre::Context;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use when_is_it::TimeAgent;

#[derive(Parser)]
#[command(about = "Slack bot for time conversion")]
struct Cli {
    /// Ollama URL
    #[arg(
        long,
        default_value = "http://localhost:11434",
        env = "WHEN_IS_IT_OLLAMA_URL"
    )]
    ollama_url: String,

    /// Ollama model to use
    #[arg(long, default_value = "qwen3:4b-instruct", env = "WHEN_IS_IT_MODEL")]
    model: String,

    /// Address to listen on
    #[arg(long, default_value = "0.0.0.0:3000", env = "WHEN_IS_IT_LISTEN")]
    listen: String,

    /// Slack signing secret for request verification
    #[arg(long, env = "WHEN_IS_IT_SLACK_SIGNING_SECRET")]
    signing_secret: String,

    /// Slack slash command name (e.g. "/when-is-it")
    #[arg(long, default_value = "/when-is-it", env = "WHEN_IS_IT_SLACK_COMMAND")]
    command: String,
}

#[derive(Clone)]
struct AppState {
    agent: Arc<TimeAgent>,
    signing_secret: String,
    command: String,
    http_client: reqwest::Client,
}

#[derive(Deserialize)]
struct SlashCommand {
    command: String,
    text: String,
    response_url: String,
    user_id: String,
}

#[derive(Serialize)]
struct SlackMessage {
    response_type: &'static str,
    text: String,
}

fn verify_signature(headers: &HeaderMap, body: &[u8], signing_secret: &str) -> Result<(), ()> {
    let timestamp = headers
        .get("X-Slack-Request-Timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or(())?;

    let signature = headers
        .get("X-Slack-Signature")
        .and_then(|v| v.to_str().ok())
        .ok_or(())?;

    let ts: i64 = timestamp.parse().map_err(|_| ())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| ())?
        .as_secs() as i64;
    if (now - ts).abs() > 300 {
        return Err(());
    }

    let sig_basestring = format!("v0:{}:{}", timestamp, String::from_utf8_lossy(body));

    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes()).map_err(|_| ())?;
    mac.update(sig_basestring.as_bytes());

    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));
    if expected != signature {
        return Err(());
    }

    Ok(())
}

async fn slash_command(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if verify_signature(&headers, &body, &state.signing_secret).is_err() {
        return StatusCode::UNAUTHORIZED;
    }

    let command: SlashCommand = match serde_urlencoded::from_bytes(&body) {
        Ok(c) => c,
        Err(_) => return StatusCode::BAD_REQUEST,
    };

    if command.command != state.command {
        return StatusCode::BAD_REQUEST;
    }

    let query = command.text;
    let response_url = command.response_url;
    let user_id = command.user_id;

    tokio::spawn(async move {
        let message = match state.agent.convert(&query).await {
            Ok(conversion) => {
                let mut text = format!(
                    "<@{}> asked: {}\n*Source:* {} ({})",
                    user_id, query, conversion.source, conversion.source_tz
                );
                for (time, tz) in &conversion.targets {
                    text.push_str(&format!("\n*{}:* {}", tz, time));
                }
                SlackMessage {
                    response_type: "in_channel",
                    text,
                }
            }
            Err(err) => SlackMessage {
                response_type: "ephemeral",
                text: format!("Error: {err}"),
            },
        };

        if let Err(err) = state
            .http_client
            .post(&response_url)
            .json(&message)
            .send()
            .await
        {
            eprintln!("failed to post to response_url: {err}");
        }
    });

    // Acknowledge immediately so Slack doesn't time out
    StatusCode::OK
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let agent = TimeAgent::new(&cli.ollama_url, &cli.model)?;

    let state = AppState {
        agent: Arc::new(agent),
        signing_secret: cli.signing_secret,
        command: cli.command,
        http_client: reqwest::Client::new(),
    };

    let app = Router::new()
        .route("/slack/commands", post(slash_command))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cli.listen)
        .await
        .wrap_err("failed to bind")?;

    eprintln!("listening on {}", cli.listen);
    axum::serve(listener, app).await.wrap_err("server error")?;

    Ok(())
}

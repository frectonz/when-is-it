use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::IntoResponse,
    routing::get,
};
use clap::Parser;
use color_eyre::eyre::Context;
use serde::{Deserialize, Serialize};
use when_is_it::{Error as AgentError, TimeAgent};

#[derive(Serialize)]
struct ErrorResponse {
    code: ErrorCode,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ErrorCode {
    MissingToken,
    InvalidToken,
    CouldNotParse,
    AmbiguousTimezone,
    MissingSourceTime,
    MissingSourceTimezone,
    MissingTargetTimezones,
    LlmFailure,
    InternalError,
}

struct AppError {
    status: StatusCode,
    body: ErrorResponse,
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(self.body)).into_response()
    }
}

impl From<AgentError> for AppError {
    fn from(err: AgentError) -> Self {
        match &err {
            AgentError::Llm(llm_err) => {
                let code = match llm_err {
                    when_is_it::LlmError::CouldNotParse => ErrorCode::CouldNotParse,
                    when_is_it::LlmError::AmbiguousTimezone => ErrorCode::AmbiguousTimezone,
                    when_is_it::LlmError::MissingSourceTime => ErrorCode::MissingSourceTime,
                    when_is_it::LlmError::MissingSourceTimezone => ErrorCode::MissingSourceTimezone,
                    when_is_it::LlmError::MissingTargetTimezones => {
                        ErrorCode::MissingTargetTimezones
                    }
                };
                AppError {
                    status: StatusCode::BAD_REQUEST,
                    body: ErrorResponse {
                        code,
                        message: err.to_string(),
                    },
                }
            }
            AgentError::Prompt(_) | AgentError::OllamaClient(_) => AppError {
                status: StatusCode::BAD_GATEWAY,
                body: ErrorResponse {
                    code: ErrorCode::LlmFailure,
                    message: err.to_string(),
                },
            },
            _ => AppError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                body: ErrorResponse {
                    code: ErrorCode::InternalError,
                    message: err.to_string(),
                },
            },
        }
    }
}

#[derive(Parser)]
#[command(about = "Web server for time conversion")]
struct Cli {
    /// Ollama base URL
    #[arg(
        long,
        default_value = "http://localhost:11434",
        env = "WHEN_IS_IT_BASE_URL"
    )]
    base_url: String,

    /// Ollama model to use
    #[arg(long, default_value = "qwen3:4b-instruct", env = "WHEN_IS_IT_MODEL")]
    model: String,

    /// Address to listen on
    #[arg(long, default_value = "0.0.0.0:3000", env = "WHEN_IS_IT_LISTEN")]
    listen: String,

    /// Bearer token for authentication (if omitted, no auth is required)
    #[arg(long, env = "WHEN_IS_IT_TOKEN")]
    token: Option<String>,
}

#[derive(Clone)]
struct AppState {
    agent: Arc<TimeAgent>,
    token: Option<String>,
}

#[derive(Deserialize)]
struct ConvertQuery {
    q: String,
}

#[derive(Serialize)]
struct ConvertResponse {
    source: String,
    source_tz: String,
    targets: Vec<TargetResponse>,
}

#[derive(Serialize)]
struct TargetResponse {
    time: String,
    tz: String,
}

async fn convert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ConvertQuery>,
) -> Result<Json<ConvertResponse>, AppError> {
    if let Some(expected) = &state.token {
        let provided = headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(AppError {
                status: StatusCode::UNAUTHORIZED,
                body: ErrorResponse {
                    code: ErrorCode::MissingToken,
                    message: "missing bearer token".into(),
                },
            })?;

        if provided != expected {
            return Err(AppError {
                status: StatusCode::UNAUTHORIZED,
                body: ErrorResponse {
                    code: ErrorCode::InvalidToken,
                    message: "invalid token".into(),
                },
            });
        }
    }

    let conversion = state.agent.convert(&query.q).await?;

    Ok(Json(ConvertResponse {
        source: conversion.source.to_string(),
        source_tz: conversion.source_tz,
        targets: conversion
            .targets
            .into_iter()
            .map(|(time, tz)| TargetResponse {
                time: time.to_string(),
                tz,
            })
            .collect(),
    }))
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let agent = TimeAgent::new(&cli.base_url, &cli.model)?;
    let state = AppState {
        agent: Arc::new(agent),
        token: cli.token,
    };

    let app = Router::new()
        .route("/convert", get(convert))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&cli.listen)
        .await
        .wrap_err("failed to bind")?;

    eprintln!("listening on {}", cli.listen);
    axum::serve(listener, app)
        .await
        .wrap_err("server error")?;

    Ok(())
}

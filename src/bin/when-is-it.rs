use clap::Parser;
use when_is_it::TimeAgent;

#[derive(Parser)]
#[command(about = "Convert times between timezones using natural language")]
struct Cli {
    /// Natural language time conversion query
    query: String,

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
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    let agent = TimeAgent::new(&cli.ollama_url, &cli.model)?;
    let conversion = agent.convert(&cli.query).await?;

    println!("Source: {} ({})", conversion.source, conversion.source_tz);
    for (target, tz) in &conversion.targets {
        println!("Target: {} ({})", target, tz);
    }

    Ok(())
}

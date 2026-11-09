mod cli;
use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match cli::execute(cli::Cli::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

mod console;
mod inspect;

use clap::{Parser, Subcommand, ValueEnum};
use layer_cake::{
    experiment::{self, ExperimentConfig},
    record::RunData,
};
use std::path::{Path, PathBuf};

pub type CliResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Parser)]
#[command(
    name = "layer-cake",
    version,
    about = "可控制、可观察、可回放的 Agent 实验"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 列出已注册实验、配置项和控制能力。
    List,
    /// 创建独立运行；--interactive 在轮次边界控制。
    Run {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        experiment: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        interactive: bool,
    },
    /// 查询保存的运行、对象、快照和完整调用。
    Show {
        run: PathBuf,
        #[arg(value_enum, default_value = "state")]
        view: View,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        version: Option<u64>,
        #[arg(long)]
        object: Option<String>,
        #[arg(long)]
        kind: Option<String>,
    },
    /// 从同一份运行记录生成自包含 HTML。
    Report {
        run: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// 比较配置、调用和上下文规模。
    Compare {
        left: PathBuf,
        right: PathBuf,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 重装原实验，以已保存的返回值替代外部调用并逐项核对请求。
    Replay { run: PathBuf },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum View {
    State,
    Objects,
    Records,
    Context,
    Calls,
}

pub fn print_json(value: &impl serde::Serialize) -> CliResult<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn write_file(path: &Path, content: &str) -> CliResult<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub async fn execute(cli: Cli) -> CliResult<()> {
    match cli.command {
        Command::List => print_json(&experiment::definitions())?,
        Command::Run {
            config,
            experiment: name,
            output,
            interactive,
        } => {
            match dotenvy::dotenv() {
                Ok(_) => {}
                Err(dotenvy::Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {
                    return Err("could not load .env; check its permissions and syntax".into());
                }
            }
            let mut config: ExperimentConfig = if let Some(path) = config {
                serde_json::from_str(&std::fs::read_to_string(path)?)?
            } else {
                ExperimentConfig::default()
            };
            if let Some(name) = name {
                config.experiment = name;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let path = output.unwrap_or_else(|| PathBuf::from(format!("runs/{id}.json")));
            let mut run = experiment::assemble(config, id.clone())?;
            println!("run={id} records={}", path.display());
            if interactive {
                console::run(run, path).await?;
            } else {
                run.log.save(&path)?;
                while !run.finished {
                    let result = run.step().await;
                    run.log.save(&path)?;
                    result?;
                }
                print_json(&run.state())?;
                if !run.normal_finish() {
                    return Err("run ended without normal completion; inspect saved state".into());
                }
            }
        }
        Command::Show {
            run,
            view,
            id,
            version,
            object,
            kind,
        } => {
            let data = RunData::load(run)?;
            print_json(&inspect::select(
                &data,
                view,
                id.as_deref(),
                version,
                object.as_deref(),
                kind.as_deref(),
            )?)?;
        }
        Command::Report { run, output } => {
            let data = RunData::load(run)?;
            write_file(&output, &layer_cake::report::html(&data)?)?;
            println!("{}", output.display());
        }
        Command::Compare {
            left,
            right,
            output,
        } => {
            let result = layer_cake::report::compare(&RunData::load(left)?, &RunData::load(right)?);
            if let Some(path) = output {
                write_file(&path, &serde_json::to_string_pretty(&result)?)?;
            } else {
                print_json(&result)?;
            }
        }
        Command::Replay { run } => {
            print_json(&layer_cake::replay::verify(&RunData::load(run)?).await?)?
        }
    }
    Ok(())
}

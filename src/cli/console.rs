use super::{CliResult, View, inspect, print_json, write_file};
use layer_cake::experiment::{ControlCommand, Experiment, launch};
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, BufReader};

const HELP: &str = "continue | step | pause | stop | input <JSON 或文本> | state | objects | context [version] | calls [record-id] | records | report <file.html> | save | quit";

pub async fn run(experiment: Experiment, path: PathBuf) -> CliResult<()> {
    let log = experiment.log.clone();
    let (control, mut notices, worker) = launch(experiment, path.clone());
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    println!("{HELP}\n暂停等待完整轮次；停止保存当前调用返回后阻止后续派发。");
    print!("> ");
    std::io::stdout().flush()?;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Some(line) = line? else {
                    control.send(ControlCommand::Shutdown).await?;
                    break;
                };
                let line = line.trim();
                let (command,args) = line.split_once(' ').unwrap_or((line,""));
                let result: CliResult<bool> = async {
                    let operation = match command {
                        "continue" => Some(ControlCommand::Continue),
                        "step" => Some(ControlCommand::Step),
                        "pause" => Some(ControlCommand::Pause),
                        "stop" => Some(ControlCommand::Stop),
                        "input" => {
                            if args.is_empty() { return Err("input requires JSON or text".into()); }
                            let value = serde_json::from_str(args).unwrap_or_else(|_| Value::String(args.into()));
                            Some(ControlCommand::Input(value))
                        }
                        "quit" | "exit" => { control.send(ControlCommand::Shutdown).await?; return Ok(true); }
                        "state" | "objects" | "context" | "calls" | "records" => {
                            let view = match command {"objects"=>View::Objects,"context"=>View::Context,"calls"=>View::Calls,"records"=>View::Records,_=>View::State};
                            let version = if command == "context" && !args.is_empty() { Some(args.parse::<u64>()?) } else { None };
                            let id = if command == "calls" && !args.is_empty() { Some(args) } else { None };
                            print_json(&inspect::select(&log.data()?,view,id,version,None,None)?)?;
                            None
                        }
                        "report" => {
                            if args.is_empty() { return Err("report requires an output path".into()); }
                            write_file(&PathBuf::from(args),&layer_cake::report::html(&log.data()?)?)?;
                            println!("{args}"); None
                        }
                        "save" => { log.save(&path)?; println!("{}",path.display()); None }
                        "help" | "?" | "" => { println!("{HELP}"); None }
                        _ => return Err(format!("unknown console command: {command}").into()),
                    };
                    if let Some(operation) = operation { control.send(operation).await?; }
                    Ok(false)
                }.await;
                match result { Ok(true)=>break,Ok(false)=>{},Err(error)=>eprintln!("error: {error}") }
                print!("> "); std::io::stdout().flush()?;
            }
            notice = notices.recv() => {
                match notice {
                    Some(notice) => { print_json(&notice)?; print!("> "); std::io::stdout().flush()?; }
                    None => break,
                }
            }
        }
    }
    // Shutdown is cooperative: awaiting the worker preserves in-flight call results.
    worker.await??;
    log.save(&path)?;
    Ok(())
}

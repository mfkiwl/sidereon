use std::path::PathBuf;

use sidereon_scoreboard::{
    default_lookback_days, parse_product_date, report_json_pretty, run_default, utc_today,
    write_report_outputs, ScoreboardError,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), ScoreboardError> {
    let cli = Cli::parse()?;
    let date = match cli.date {
        Some(value) => parse_product_date(&value)?,
        None => utc_today()?,
    };
    let report = run_default(date, cli.lookback_days)?;
    write_report_outputs(&report, cli.output.as_deref(), cli.history.as_deref())?;
    println!("{}", report_json_pretty(&report)?);
    Ok(())
}

#[derive(Debug)]
struct Cli {
    output: Option<PathBuf>,
    history: Option<PathBuf>,
    date: Option<String>,
    lookback_days: u32,
}

impl Cli {
    fn parse() -> Result<Self, ScoreboardError> {
        let mut output = Some(PathBuf::from("latest.json"));
        let mut history = Some(PathBuf::from("history.jsonl"));
        let mut date = None;
        let mut lookback_days = default_lookback_days();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--output" => output = Some(next_path(&mut args, "--output")?),
                "--history" => history = Some(next_path(&mut args, "--history")?),
                "--date" => date = Some(next_string(&mut args, "--date")?),
                "--lookback-days" => {
                    lookback_days = next_string(&mut args, "--lookback-days")?
                        .parse::<u32>()
                        .map_err(|_| {
                            ScoreboardError::InvalidArgument(
                                "--lookback-days must be an integer".to_string(),
                            )
                        })?;
                }
                "--stdout-only" => {
                    output = None;
                    history = None;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    return Err(ScoreboardError::InvalidArgument(format!(
                        "unknown argument {other}"
                    )));
                }
            }
        }
        Ok(Self {
            output,
            history,
            date,
            lookback_days,
        })
    }
}

fn next_path(
    args: &mut impl Iterator<Item = String>,
    flag: &'static str,
) -> Result<PathBuf, ScoreboardError> {
    Ok(PathBuf::from(next_string(args, flag)?))
}

fn next_string(
    args: &mut impl Iterator<Item = String>,
    flag: &'static str,
) -> Result<String, ScoreboardError> {
    args.next()
        .ok_or_else(|| ScoreboardError::InvalidArgument(format!("{flag} requires a value")))
}

fn print_help() {
    println!(
        "Usage: sidereon-scoreboard [--output latest.json] [--history history.jsonl] [--date YYYY-MM-DD] [--lookback-days N] [--stdout-only]"
    );
}

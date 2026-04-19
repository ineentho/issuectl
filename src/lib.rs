mod app;
mod cli;
mod db;
mod domain;
mod error;
mod git;
mod output;
mod repo;

use clap::Parser;

use crate::app::App;
use crate::cli::Cli;
use crate::error::{CliError, emit_error, exit_code};

pub fn run_cli() -> i32 {
    let cli = Cli::parse();

    match run(cli) {
        Ok(()) => 0,
        Err(err) => {
            let code = exit_code(&err);
            emit_error(err.json_mode(), &err, code);
            code
        }
    }
}

fn run(cli: Cli) -> Result<(), CliError> {
    let json_output = cli.json;
    let app = App::new(json_output)?;
    app.dispatch(cli.command)
}

use clap::Parser;
use domaingrep::{cli::Cli, run, RuntimeConfig};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let exit_code = match RuntimeConfig::from_env() {
        Ok(config) => match run(cli, config).await {
            Ok(report) => {
                if !report.stdout.is_empty() {
                    print!("{}", report.stdout);
                }
                for line in report.stderr {
                    eprintln!("{line}");
                }
                report.exit_code
            }
            Err(err) => {
                eprint!("{err}");
                err.exit_code()
            }
        },
        Err(err) => {
            eprint!("{err}");
            err.exit_code()
        }
    };

    std::process::exit(exit_code);
}

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorWhen {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "domaingrep",
    version,
    about = "Bulk domain availability search CLI tool"
)]
pub struct Cli {
    #[arg(long, short = 'a')]
    pub all: bool,

    #[arg(long, short = 'j')]
    pub json: bool,

    #[arg(long, short = 't', value_name = "RANGE")]
    pub tld_len: Option<String>,

    #[arg(long, short = 'l', value_name = "N")]
    pub limit: Option<usize>,

    #[arg(long, value_enum, default_value_t = ColorWhen::Auto)]
    pub color: ColorWhen,

    pub domain: Option<String>,
}

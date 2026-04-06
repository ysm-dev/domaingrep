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
    #[arg(long, short = 'a', help = "Show unavailable domains too")]
    pub all: bool,

    #[arg(long, short = 'j', help = "Output as NDJSON")]
    pub json: bool,

    #[arg(
        long,
        short = 't',
        value_name = "RANGE",
        help = "Filter TLDs by length: 2, 2..5, ..3, 4.."
    )]
    pub tld_len: Option<String>,

    #[arg(
        long,
        short = 'l',
        value_name = "N",
        help = "Maximum rows to emit after filtering; default 25 in terminal, 0 shows all"
    )]
    pub limit: Option<usize>,

    #[arg(long, value_enum, default_value_t = ColorWhen::Auto, help = "Color output: auto, always, never")]
    pub color: ColorWhen,

    #[arg(
        value_name = "DOMAIN",
        required = true,
        help = "Domain to search: 'abc' or 'abc.sh'"
    )]
    pub domain: Option<String>,
}

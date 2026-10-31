//! Command-line entry point.

use clap::Parser;

/// Command-line interface for dalsegno.
#[derive(Debug, Parser)]
#[command(name = "dalsegno", version, about)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    Cli::parse();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn version_flag_is_reported() {
        let err = Cli::try_parse_from(["dalsegno", "--version"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayVersion);
    }
}

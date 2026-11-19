use clap::Parser;

/// Wallpaper-driven theming for macOS.
#[derive(Parser)]
#[command(version, about)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}

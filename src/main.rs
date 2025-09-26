use std::{env::temp_dir, fs::File};

use anyhow::Result;
use clap::Parser;
use gix_of_theseus::{formatter, plot, theseus};

#[derive(Debug, clap::Parser)]
#[clap(
    name = "gix-of-theseus",
    about = "Collects and plots historical data about the composition of git repositories.",
    version
)]
struct Cli {
    #[clap(subcommand)]
    subcommand: Subcommands,
}

#[derive(Debug, Parser)]
pub struct PlotArgs {
    #[clap(short, long)]
    input_file: String,
    #[clap(short, long)]
    output_file: String,
}
#[derive(Debug, Parser)]
struct TheseusArgs {
    repo_path: String,
    #[clap(short, long)]
    image_file: Option<String>,
}

#[derive(Debug, clap::Subcommand)]
enum Subcommands {
    //Just run the plot script on a cohorts.json file
    Plot(PlotArgs),
    /// Collect the data and optionally plot a chart for a repo.
    Theseus(TheseusArgs),
}

fn main() -> Result<()> {
    let args = Cli::parse();
    match args.subcommand {
        Subcommands::Plot(args) => plot::run_stackplot(args.input_file, args.output_file),
        Subcommands::Theseus(args) => {
            let res = theseus::run_theseus(&args.repo_path).expect("Error running theseus");
            let formatted_data = formatter::format_cohort_data(res);
            let repo_last_part = args.repo_path.split('/').last().unwrap();
            let temp_file = temp_dir().join(format!("{}.json", repo_last_part));
            println!("Writing to {}", temp_file.display());
            serde_json::to_writer_pretty(File::create(temp_file.clone())?, &formatted_data)?;
            let image_file = args.image_file.unwrap_or(format!("{}.png", repo_last_part));
            plot::run_stackplot(temp_file.display().to_string(), image_file)?;
            Ok(())
        }
    }
}

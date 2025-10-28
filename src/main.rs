use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

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
pub struct AnalyzeArgs {
    #[clap(short, long)]
    input_file: String,
    #[clap(short, long)]
    output_file: String,
}
#[derive(Debug, Parser)]
struct TheseusArgs {
    repo_path: String,
    #[clap(short, long)]
    outdir: Option<PathBuf>,
    #[clap(short, long)]
    no_plot: bool,
    #[clap(short, long, default_value = "false")]
    all_filetypes: bool,
}

#[derive(Debug, clap::Subcommand)]
enum Subcommands {
    /// Plot the data in a cohorts.json file
    Plot(PlotArgs),
    /// Analyze a repo's contents and write the data to a cohorts.json file, and optionally plot it
    Analyze(TheseusArgs),
}

fn analyze_repo(repo_path: &str, outdir: PathBuf) -> Result<PathBuf> {
    let res = theseus::run_theseus(repo_path).expect("Error running theseus");
    let formatted_data = formatter::format_cohort_data(res);
    let cohorts_file = outdir.join("cohorts.json");
    println!("Writing cohort data to {}", cohorts_file.display());
    serde_json::to_writer_pretty(File::create(cohorts_file.clone())?, &formatted_data)?;
    Ok(cohorts_file)
}
fn main() -> Result<()> {
    let args = Cli::parse();
    match args.subcommand {
        Subcommands::Plot(args) => plot::run_stackplot(args.input_file, args.output_file, None),
        Subcommands::Analyze(args) => {
            let python_runner = plot::get_python_runner();
            let repo_path = Path::new(&args.repo_path);
            let repo_name = repo_path.file_name().unwrap().to_str().unwrap();

            let outdir = args.outdir.unwrap_or_else(|| PathBuf::from(repo_name));
            fs::create_dir_all(&outdir)?;
            let cohorts_file =
                analyze_repo(&args.repo_path, outdir.clone()).expect("Error analyzing repo");
            if !args.no_plot {
                if python_runner.is_some() {
                    let image_file = outdir.join("stackplot.png");
                    plot::run_stackplot(
                        cohorts_file.display().to_string().clone(),
                        image_file.display().to_string(),
                        Some(repo_name.to_string()),
                    )?;
                } else {
                    println!(
                        "No Python PEP 723 script runner found (tried: uv, pipx), we won't be able to plot the chart automatically and will only save the raw to cohorts.json.\nYou can install uv with `pip install uv` or pipx with `pip install pipx`"
                    );
                }
            }
            Ok(())
        }
    }
}

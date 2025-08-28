use clap::Parser;
use gix_of_theseus::run_theseus;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    repo_path: String,
}

fn main() {
    let args = Args::parse();
    match run_theseus(&args.repo_path) {
        Ok(results) => {
            println!("{:?}", results);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
        }
    }
}

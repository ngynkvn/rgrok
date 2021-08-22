use clap::Clap;
use rgrok::{rgrok_dir, rgrok_dir_parallel, Args};

use color_eyre::Result;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;

fn main() -> Result<()> {
    color_eyre::install()?;
    // Load these once at the start of your program
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();

    let args = Args::parse();

    if args.parallel {
        rgrok_dir_parallel(args, &ps, &ts)
    } else {
        rgrok_dir(args, &ps, &ts)
    }
}

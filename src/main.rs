use syntect::util::as_24_bit_terminal_escaped;
use syntect::highlighting::Style;
use syntect::util::LinesWithEndings;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use std::process::Stdio;
use std::io::Stdin;
use std::io::Write;
use std::process::Command;
use syn::__private::ToTokens;
use syn::Item::Fn;
use ignore::types::TypesBuilder;
use std::path::PathBuf;
use clap::Clap;
use ignore::Walk;

#[derive(Clap, Clone)]
struct Args {
    regex: Option<String>,
    #[clap(short, long, default_value=".")]
    path: PathBuf 
}
fn rustfmt(string: String) -> String {
    let mut process = Command::new("rustfmt").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = process.stdin.take().expect("Failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(string.as_bytes()).unwrap();
    });
    let output = process.wait_with_output().unwrap().stdout;
    String::from_utf8_lossy(&output).into()
}

fn main() {

    // Load these once at the start of your program
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();

    let syntax = ps.find_syntax_by_extension("rs").unwrap();
    let args = Args::parse();
    for file in Walk::new(args.path) {
        match file {
            Ok(dir_entry) => {
                if dir_entry.metadata().map(|m| !m.is_dir()).unwrap_or(false) 
                && dir_entry.path().extension().unwrap_or_default() == "rs" { // Is rust file
                    let contents = std::fs::read_to_string(dir_entry.path()).unwrap();
                    let file = syn::parse_file(&contents).unwrap();
                    for function in file.items.iter() {
                        if let Fn(_) = function {
                            let fn_string = function.to_token_stream().to_string();
                            let output = rustfmt(fn_string);
                            // Print highlighted strings.
                            let mut h = syntect::easy::HighlightLines::new(syntax, &ts.themes["base16-ocean.dark"]);
                            for line in LinesWithEndings::from(&output) {
                                let ranges: Vec<(Style, &str)> = h.highlight(line, &ps);
                                let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
                                print!("{}", escaped);
                            }
                        }
                    }
                }
            },
            _ => {}
        }
    }
}

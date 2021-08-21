pub mod util;

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
    thread::{self, JoinHandle},
};

use color_eyre::{
    eyre::{self, Context},
    Result,
};
use crossbeam::channel::{select, Receiver, Sender};
use crossbeam::thread::Scope;
use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, Walk, WalkBuilder, WalkParallel};
use regex::Regex;
use syn::__private::ToTokens;
use syntect::{
    highlighting::{Color, FontStyle, Style, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
    util::{as_24_bit_terminal_escaped, LinesWithEndings},
};

use util::parse_file;
use util::print_header_info;
use util::ParsedFile;

use clap::Clap;
use color_eyre::Report;
#[derive(Clap, Clone)]
pub struct Args {
    pub regex: regex::Regex,
    #[clap(short, long, default_value = ".")]
    pub path: PathBuf,
    #[clap(long)]
    pub parallel: bool,
    #[clap(long, default_value = "stdout")]
    pub output: Output,
}

impl FromStr for Output {
    type Err = Report;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "stdout" => Ok(Output::Stdout),
            "null" => Ok(Output::Null),
            _ => Err(eyre::eyre!("Invalid output")),
        }
    }
}

#[derive(Clone)]
pub enum Output {
    Stdout,
    Null,
}
impl std::io::Write for Output {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Stdout => std::io::stdout().write(buf),
            Self::Null => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Stdout => std::io::stdout().flush(),
            Self::Null => Ok(()),
        }
    }
}

/// Shell out to rustfmt for code presentation.
/// Unfortunately, rustfmt is not designed to be used as a library, so we have to spawn a process to do the formatting for us.
pub fn rustfmt(string: String) -> Result<String> {
    let mut process = Command::new("rustfmt")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    {
        let stdin = process
            .stdin
            .as_mut()
            .ok_or(eyre::eyre!("Unable to obtain handle to stdin process."))?;
        stdin.write_all(string.as_bytes())?;
    }
    let output = process.wait_with_output()?.stdout;
    Ok(String::from_utf8_lossy(&output).into())
}

pub fn rgrok_dir(mut args: Args, ps: &SyntaxSet, ts: &ThemeSet) -> Result<()> {
    for file in Walk::new(args.path) {
        match file {
            Ok(dir_entry) => {
                if is_rust_file(&dir_entry) {
                    let syntax = ps.find_syntax_for_file(dir_entry.path())?.ok_or_else(|| {
                        eyre::eyre!(
                            "Syntax highlight support was not found for the following file: {:?}",
                            dir_entry.path()
                        )
                    })?;
                    let file = parse_file(dir_entry)?;
                    grep_items(&mut args.output, &file, &args.regex, syntax, &ps, &ts);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn rgrok_dir_parallel(mut args: Args, ps: &SyntaxSet, ts: &ThemeSet) -> Result<()> {
    let walker = WalkBuilder::new(args.path).threads(0).build_parallel();

    struct Visitor<'a> {
        tx: Sender<(ParsedFile, SyntaxReference)>,
        quit: Sender<()>,
        ps: &'a SyntaxSet,
        re: &'a regex::Regex,
    }
    impl<'a> ParallelVisitor for Visitor<'a> {
        fn visit(&mut self, entry: Result<DirEntry, ignore::Error>) -> ignore::WalkState {
            use ignore::WalkState::*;
            match entry {
                Ok(dir_entry) => {
                    if is_rust_file(&dir_entry) {
                        let syntax = self
                            .ps
                            .find_syntax_for_file(dir_entry.path())
                            .wrap_err(eyre::eyre!(
                            "Syntax highlight support was not found for the following file: {:?}",
                            dir_entry.path()
                            ))
                            .unwrap()
                            .unwrap();
                        let file = parse_file(dir_entry).unwrap();
                        if self.re.is_match(&file.contents) {
                            self.tx.send((file, syntax.clone())).unwrap();
                        }
                    }
                    Continue
                }
                Err(_) => Continue,
            }
        }
    }
    impl<'a> Drop for Visitor<'a> {
        fn drop(&mut self) {
            self.quit.send(()).unwrap()
        }
    }
    struct VisitorBuilder<'a> {
        ps: &'a SyntaxSet,
        re: &'a regex::Regex,
        tx: Sender<(ParsedFile, SyntaxReference)>,
        quit: Sender<()>,
        thread_count: usize,
    }
    impl<'s> ParallelVisitorBuilder<'s> for VisitorBuilder<'s> {
        fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
            self.thread_count += 1;
            Box::new(Visitor {
                tx: self.tx.clone(),
                quit: self.quit.clone(),
                ps: self.ps,
                re: self.re,
            })
        }
    }

    let (tx, rx) = crossbeam::channel::unbounded::<(ParsedFile, SyntaxReference)>();
    let (quit, done) = crossbeam::channel::unbounded::<()>();
    let mut vbuilder = VisitorBuilder {
        ps: &ps,
        re: &args.regex,
        tx,
        quit,
        thread_count: 0,
    };
    walker.visit(&mut vbuilder);

    let mut threads_finished = 0;
    loop {
        select! {
            recv(done) -> _ => {
                threads_finished += 1;
                if threads_finished == vbuilder.thread_count {
                    break
                }
            }
            recv(rx) -> msg => {
                match msg {
                    Ok((file, syntax)) => {
                        grep_items(&mut args.output, &file, &args.regex, &syntax, &ps, &ts)
                    },
                    _ => panic!()
                }
            }
        }
    }

    Ok(())
}

pub fn is_rust_file(dir_entry: &DirEntry) -> bool {
    dir_entry.metadata().map(|m| !m.is_dir()).unwrap_or(false)
        && dir_entry.path().extension().unwrap_or_default() == "rs"
}

pub fn grep_items(
    output: &mut Output,
    file: &ParsedFile,
    re: &Regex,
    syntax: &SyntaxReference,
    ps: &SyntaxSet,
    ts: &ThemeSet,
) {
    let syn_file = syn::parse_file(&file.contents)
        .wrap_err_with(|| eyre::eyre!("Unable to parse \"{}\"", file.dir_entry.path().display()))
        .unwrap();
    for item in syn_file.items.iter() {
        let item_str = item.to_token_stream().to_string();
        let output_str = rustfmt(item_str).unwrap();
        if re.is_match(&output_str) {
            // Print header information for file.
            print_header_info(output, &file, item);
            // Print highlighted strings.
            let mut h = syntect::easy::HighlightLines::new(syntax, &ts.themes["base16-ocean.dark"]);

            for line in LinesWithEndings::from(&output_str) {
                let mut ranges: Vec<(Style, &str)> = h.highlight(line, ps);
                highlight_matches_in_line(&mut ranges, re.find_iter(line));
                let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
                write!(output, "{}", escaped).unwrap();
            }
            writeln!(output, "\x1b[0m").unwrap();
        }
    }
}

const HIGHLIGHT_COLOR: Color = Color {
    r: 255,
    g: 255,
    b: 80,
    a: 255,
};

/// Modify the output vec to highlight the regex matches found in the iter.
pub fn highlight_matches_in_line(ranges: &mut Vec<(Style, &str)>, line_matches: regex::Matches) {
    // Index ranges for currently styled slices.
    let mut rs = vec![0usize];
    rs.extend(ranges.iter().scan(0, |acc, (_, slice)| {
        *acc += slice.len();
        Some(*acc)
    }));

    for m in line_matches {
        match rs.binary_search(&m.start()) {
            Ok(index) => {
                ranges[index].0.font_style = FontStyle::BOLD;
                ranges[index].0.foreground = HIGHLIGHT_COLOR;
            }
            Err(new_index) => {
                if rs[new_index] == m.end() {
                    // Found end of target string.
                    if rs.get(new_index - 1).is_some() {
                        ranges[new_index - 1].0.font_style = FontStyle::BOLD;
                        ranges[new_index - 1].0.foreground = HIGHLIGHT_COLOR;
                    }
                }
            }
        }
    }
}
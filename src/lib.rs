pub mod util;

use syn::spanned::Spanned;
use syn::Item;
use crate::util::item_type;
use crate::util::ItemType;

use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
};

use color_eyre::{
    eyre::{self, Context},
    Result,
};
use crossbeam::channel::{select, Sender};

use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, Walk, WalkBuilder};
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
                    grep_items(&mut args.output, &file, &args.regex, syntax, ps, ts);
                } else {
                    // ?
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
        ps,
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
                        grep_items(&mut args.output, &file, &args.regex, &syntax, ps, ts)
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


use lazy_static::lazy_static;

lazy_static! {
    static ref LINE_REGEX: regex::Regex = regex::Regex::new("(.*)").unwrap();
}
pub fn grep_items(
    output: &mut Output,
    file: &ParsedFile,
    re: &Regex,
    syntax: &SyntaxReference,
    ps: &SyntaxSet,
    ts: &ThemeSet,
) {
    let syn_file: syn::File;
    match syn::parse_file(&file.contents) {
        Ok(f) => syn_file = f,
        Err(_) => return,
    };
    let mut byte_spans = vec![0usize];
    byte_spans.extend(LINE_REGEX.find_iter(&file.contents).scan(0, |acc, l| {
        *acc += l.as_str().len();
        Some(*acc)
    }));
    let rx = {
        let (tx, rx) = crossbeam::channel::unbounded();
        use rayon::prelude::*;
        syn_file
            .items
            .iter()
            .map(|item| {
                let span = item.span();
                let (start, end) = (span.start().line, span.end().line);
                let item_type = item_type(&item);
                (item_type, (start,end))
            })
            .collect::<Vec<(ItemType, (usize, usize))>>()
            .into_par_iter()
            .for_each(|(t, (start, end))| {
                let string = Vec::new();
                let mut writer = std::io::BufWriter::new(string);
                let num_skip: usize = byte_spans[start];
                let end: usize = byte_spans[end];
                let item = &file.contents[num_skip..end];
                if re.is_match(item) {
                    let mut h =
                        syntect::easy::HighlightLines::new(syntax, &ts.themes["base16-ocean.dark"]);
                    // Print header information for file.
                    print_header_info(&mut writer, file, t);
                    // Print highlighted strings.
                    for line in LinesWithEndings::from(item) {
                        let mut ranges: Vec<(Style, &str)> = h.highlight(line, ps);
                        highlight_matches_in_line(&mut ranges, re.find_iter(line));
                        // morph_ranges(&mut ranges, output_str);
                        let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
                        write!(writer, "{}", escaped).unwrap();
                    }
                    writeln!(writer, "\x1b[0m").unwrap();
                }
                tx.send(writer).unwrap();
            });
        rx
    };
    loop {
        match rx.recv() {
            Ok(writer) => {
                write!(
                    output,
                    "{}",
                    String::from_utf8(writer.into_inner().unwrap()).unwrap()
                ).unwrap();
            }
            Err(_) => break,
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

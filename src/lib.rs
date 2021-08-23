pub mod util;

use crate::util::item_type;
use crate::util::ItemType;

use std::io::BufWriter;
use syn::spanned::Spanned;

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

use syntect::{
    highlighting::{Color, FontStyle, Style, ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
    util::{as_24_bit_terminal_escaped, LinesWithEndings},
};

use util::parse_file;

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
impl Compositor for Output {
    type Context = ((usize, usize), ItemType);
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

use crossterm::terminal;

struct TerminalPrinter {
    size: (u16, u16),
    output: Output,
    line: String,
}

impl TerminalPrinter {
    pub fn new(output: Output) -> Result<Self, std::io::Error> {
        let (x, y) = terminal::size()?;
        Ok(Self {
            size: (x, y),
            output,
            line: "-".repeat(x as _),
        })
    }
}

impl Write for TerminalPrinter {
    /// Composites a simple line frame around the buffer.
    fn write(&mut self, buf: &[u8]) -> std::result::Result<usize, std::io::Error> {
        match &self.output {
            Output::Stdout => {
                let mut stdout = std::io::stdout();
                stdout.write(buf)
            }
            Output::Null => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::result::Result<(), std::io::Error> {
        self.output.flush()
    }
}

impl Compositor for TerminalPrinter {
    type Context = ((usize, usize), ItemType);
    fn write_with(
        &mut self,
        args: std::fmt::Arguments,
        ((start, end), item_type): Self::Context,
    ) -> std::result::Result<(), std::io::Error> {
        writeln!(self.output, "{}", self.line)?;
        writeln!(self.output, "{:?}, ({}, {})", item_type, start, end)?;
        writeln!(self.output, "{}", self.line)?;
        let result = self.write_fmt(args);
        writeln!(self.output, "{}", self.line)?;
        writeln!(self.output)?;

        result
    }
}

/// A compositor is just a fancy writer that can understand some more contextual information.
pub trait Compositor: Write {
    type Context;
    fn write_with(
        &mut self,
        args: std::fmt::Arguments,
        _: Self::Context,
    ) -> std::result::Result<(), std::io::Error> {
        self.write_fmt(args)
    }
}

pub fn rgrok_dir(args: Args, ps: &SyntaxSet, ts: &ThemeSet) -> Result<()> {
    let mut printer = TerminalPrinter::new(args.output)?;
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
                    grep_items(&mut printer, &file, &args.regex, syntax, ps, ts);
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
    struct VisitorBuilder<'a> {
        ps: &'a SyntaxSet,
        re: &'a regex::Regex,
        tx: Sender<(ParsedFile, SyntaxReference)>,
    }
    impl<'s> ParallelVisitorBuilder<'s> for VisitorBuilder<'s> {
        fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
            Box::new(Visitor {
                tx: self.tx.clone(),
                ps: self.ps,
                re: self.re,
            })
        }
    }

    let (tx, rx) = crossbeam::channel::unbounded::<(ParsedFile, SyntaxReference)>();

    {
        let mut vbuilder = VisitorBuilder {
            ps,
            re: &args.regex,
            tx,
        };
        walker.visit(&mut vbuilder);
        // Drop that vbuilder
    }

    while let Ok((file, syntax)) = rx.recv() {
        grep_items(&mut args.output, &file, &args.regex, &syntax, ps, ts)
    }

    Ok(())
}

pub fn is_rust_file(dir_entry: &DirEntry) -> bool {
    dir_entry.metadata().map(|m| !m.is_dir()).unwrap_or(false)
        && dir_entry.path().extension().unwrap_or_default() == "rs"
}

use lazy_static::lazy_static;

struct GrepResult {
    line_range: (usize, usize),
    item_type: ItemType,
    writer: BufWriter<Vec<u8>>,
}

lazy_static! {
    static ref LINE_REGEX: regex::Regex = regex::RegexBuilder::new("(.*\r?\n?)")
        .multi_line(true)
        .build()
        .unwrap();
}
pub fn grep_items<W: Compositor<Context = ((usize, usize), ItemType)>>(
    output: &mut W,
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
    // Indexes for the starting byte offset for a given line.
    // byte_spans[i]..byte_spans[i+1] = byte range for a line in a file.
    let mut byte_spans = vec![0usize];
    byte_spans.extend(LINE_REGEX.find_iter(&file.contents).scan(0, |acc, l| {
        *acc += l.as_str().len();
        Some(*acc)
    }));
    assert_eq!(
        byte_spans.last().unwrap(),
        &file.contents.len(),
        "{}",
        file.dir_entry.path().display()
    );
    let (tx, rx) = crossbeam::channel::unbounded();
    use rayon::prelude::*;
    syn_file
        .items
        .iter()
        .map(|item| {
            let span = item.span();
            let (start, end) = (span.start().line, span.end().line);
            let item_type = item_type(item);
            (item_type, (start, end))
        })
        .collect::<Vec<(ItemType, (usize, usize))>>()
        .into_par_iter()
        .for_each(|(t, (start, end))| {
            let string = Vec::new();
            let mut writer = std::io::BufWriter::new(string);
            // println!("{} {} {}", file.dir_entry.path().display(), start, end);
            let span_start: usize = byte_spans[start];
            let span_end: usize = byte_spans[end];
            let item = &file.contents[span_start..span_end];
            if byte_spans.len() > 100 {
                // TODO
                for m in re.find_iter(item) {
                    match byte_spans.binary_search(&m.start()) {
                        Ok(_i) => {}  // The match is at a new line
                        Err(_i) => {} // The match is somewhere in i - 1 (?)
                    }
                }
            } else if re.is_match(item) {
                let mut h =
                    syntect::easy::HighlightLines::new(syntax, &ts.themes["base16-ocean.dark"]);
                // Write highlighted strings to buffer.
                for line in LinesWithEndings::from(item) {
                    let mut ranges: Vec<(Style, &str)> = h.highlight(line, ps);
                    highlight_matches_in_line(&mut ranges, re.find_iter(line));
                    let escaped = as_24_bit_terminal_escaped(&ranges[..], true);
                    write!(writer, "{}", escaped).unwrap();
                }
                writeln!(writer, "\x1b[0m").unwrap();
                let grep_result = GrepResult {
                    item_type: t,
                    line_range: (start, end),
                    writer,
                };
                tx.send(grep_result).unwrap();
            }
        });
    // Drop the unused tx after sending them to rayon iters.
    drop(tx);

    while let Ok(GrepResult {
        writer,
        line_range,
        item_type,
    }) = rx.recv()
    {
        let string = String::from_utf8(writer.into_inner().unwrap()).unwrap();
        output
            .write_with(format_args!("{}", string), (line_range, item_type))
            .unwrap();
        output.flush().unwrap();
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

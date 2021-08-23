use crate::grep_items;
use crate::is_rust_file;
use crate::parse_file;


use crate::util::ParsedFile;
use crate::Args;






use color_eyre::{
    eyre::{self, Context},
    Result,
};
use crossbeam::channel::{Sender};

use ignore::{DirEntry, ParallelVisitor, ParallelVisitorBuilder, WalkBuilder};


use syntect::{
    highlighting::{ThemeSet},
    parsing::{SyntaxReference, SyntaxSet},
};




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

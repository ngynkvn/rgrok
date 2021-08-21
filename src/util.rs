use ignore::DirEntry;

use color_eyre::{
    eyre::{self, Context},
    Result,
};

use crate::Output;
use std::io::Write;

pub struct ParsedFile {
    pub contents: String,
    pub dir_entry: DirEntry,
}

pub fn print_header_info(output: &mut Output, file: &ParsedFile, item: &syn::Item) {
    let item_type = match item {
        syn::Item::Const(_) => "const",
        syn::Item::Enum(_) => "enum",
        syn::Item::ExternCrate(_) => "extern",
        syn::Item::Fn(_) => "function",
        syn::Item::ForeignMod(_) => "foreign",
        syn::Item::Impl(_) => "impl",
        syn::Item::Macro(_) => "macro",
        syn::Item::Macro2(_) => "macro2",
        syn::Item::Mod(_) => "mod",
        syn::Item::Static(_) => "static",
        syn::Item::Struct(_) => "struct",
        syn::Item::Trait(_) => "trait",
        syn::Item::TraitAlias(_) => "trait alias",
        syn::Item::Type(_) => "type",
        syn::Item::Union(_) => "union",
        syn::Item::Use(_) => "use",
        syn::Item::Verbatim(_) => "verbatim",
        syn::Item::__TestExhaustive(_) => unreachable!(),
    };
    writeln!(
        output,
        "[{}] {}",
        item_type,
        file.dir_entry.path().display()
    )
    .unwrap();
}

pub fn parse_file(dir_entry: DirEntry) -> Result<ParsedFile> {
    let contents = std::fs::read_to_string(dir_entry.path())?;
    Ok(ParsedFile {
        contents,
        dir_entry,
    })
}

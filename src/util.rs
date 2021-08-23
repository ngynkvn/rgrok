
use ignore::DirEntry;
use std::fmt::Display;
use syn::Item;

use color_eyre::Result;

use std::io::Write;

pub struct ParsedFile {
    pub contents: String,
    pub dir_entry: DirEntry,
}

#[derive(Debug)]
pub enum ItemType {
    Fn,
    Enum,
    Const,
    ExternCrate,
    ForeignMod,
    Impl,
    Macro,
    Macro2,
    Mod,
    Static,
    Struct,
    Trait,
    TraitAlias,
    Type,
    Union,
    Use,
    Verbatim,
}

pub fn item_type(item: &Item) -> ItemType {
    match item {
        Item::Const(_) => ItemType::Const,
        Item::Enum(_) => ItemType::Enum,
        Item::ExternCrate(_) => ItemType::ExternCrate,
        Item::Fn(_) => ItemType::Fn,
        Item::ForeignMod(_) => ItemType::ForeignMod,
        Item::Impl(_) => ItemType::Impl,
        Item::Macro(_) => ItemType::Macro,
        Item::Macro2(_) => ItemType::Macro2,
        Item::Mod(_) => ItemType::Mod,
        Item::Static(_) => ItemType::Static,
        Item::Struct(_) => ItemType::Struct,
        Item::Trait(_) => ItemType::Trait,
        Item::TraitAlias(_) => ItemType::TraitAlias,
        Item::Type(_) => ItemType::Type,
        Item::Union(_) => ItemType::Union,
        Item::Use(_) => ItemType::Use,
        Item::Verbatim(_) => ItemType::Verbatim,
        Item::__TestExhaustive(_) => unreachable!(),
    }
}

impl Display for ItemType {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        write!(fmt, "{:?}", self)
    }
}

pub fn print_header_info<W: Write>(output: &mut W, file: &ParsedFile, item_type: ItemType) {
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

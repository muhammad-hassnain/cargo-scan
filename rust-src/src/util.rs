/*
    Utility
*/

use std::fmt::Display;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::vec;

use walkdir::{DirEntry, WalkDir};

/*
    Iterators
*/

pub fn infer_crate(filepath: &Path) -> String {
    let crate_src: Vec<String> = filepath
        .iter()
        .map(|x| {
            x.to_str().unwrap_or_else(|| {
                panic!("found path that wasn't a valid UTF-8 string: {:?}", x)
            })
        })
        .take_while(|&x| x != "src")
        .map(|x| x.to_string())
        .collect();
    let crate_string = crate_src.last().cloned().unwrap_or_else(|| {
        eprintln!("warning: unable to infer crate from path: {:?}", filepath);
        "".to_string()
    });
    crate_string
}

pub fn infer_module(filepath: &Path) -> Vec<String> {
    let post_src: Vec<String> = filepath
        .iter()
        .map(|x| {
            x.to_str().unwrap_or_else(|| {
                panic!("found path that wasn't a valid UTF-8 string: {:?}", x)
            })
        })
        .skip_while(|&x| x != "src")
        .skip(1)
        .filter(|&x| x != "main.rs" && x != "lib.rs")
        .map(|x| x.replace(".rs", ""))
        .collect();
    post_src
}

pub fn fully_qualified_prefix(filepath: &Path) -> String {
    let mut prefix_vec = vec![infer_crate(filepath)];
    let mut mod_vec = infer_module(filepath);
    prefix_vec.append(&mut mod_vec);
    prefix_vec.join("::")
}

/// Ignore errors, printing them to stderr
/// useful with iter::filter_map: `my_iter.filter_map(warn_ok)`
pub fn warn_ok<T, E: Display>(x: Result<T, E>) -> Option<T> {
    if let Some(e) = x.as_ref().err() {
        eprintln!("Warning: discarding error {}", e);
    }
    x.ok()
}

/// Convert an iterator into one that owns all its elements
pub trait FreshIter {
    type Result: Iterator;
    fn fresh_iter(self) -> Self::Result;
}
impl<I: Iterator> FreshIter for I {
    type Result = vec::IntoIter<I::Item>;
    fn fresh_iter(self) -> Self::Result {
        self.collect::<Vec<I::Item>>().into_iter()
    }
}

/*
    Filesystem util
*/

pub fn walk_files(p: &PathBuf) -> impl Iterator<Item = PathBuf> {
    debug_assert!(p.is_dir());
    WalkDir::new(p)
        .sort_by_file_name()
        .into_iter()
        .filter_map(warn_ok)
        .map(DirEntry::into_path)
}

pub fn file_lines(p: &PathBuf) -> impl Iterator<Item = String> {
    let file = File::open(p).unwrap();
    let reader = BufReader::new(file).lines();
    reader.map(|line| line.unwrap())
}

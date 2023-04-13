//! Interface for name resolution for Rust identifiers.
//!
//! The type FileResolver is a wrapper around Resolver from name_resolution.rs
//! with the needed functionality.

use super::ident::{CanonicalPath, Path};

use anyhow::{Context, Result};
use std::path::Path as FilePath;
use syn;

/// Common interface for FileResolver and HackyResolver
///
/// Abstracts the functionality for resolution that is needed by Scanner.
pub trait Resolve<'a>: Sized {
    /*
        Constructor and invariant
    */
    fn assert_top_level_invariant(&self);

    /*
        Resolution functions
    */
    fn resolve_ident(&self, i: &'a syn::Ident) -> Path;
    fn resolve_path(&self, p: &'a syn::Path) -> Path;
    fn resolve_def(&self, i: &'a syn::Ident) -> CanonicalPath;
    fn resolve_ffi(&self, p: &syn::Path) -> Option<CanonicalPath>;

    /*
        Optional helper functions to inform the resolver of the scope
    */
    fn push_mod(&mut self, mod_ident: &'a syn::Ident);
    fn pop_mod(&mut self);
    fn push_impl(&mut self, impl_stmt: &'a syn::ItemImpl);
    fn pop_impl(&mut self);
    fn push_fn(&mut self, fn_ident: &'a syn::Ident);
    fn pop_fn(&mut self);
    fn scan_use(&mut self, use_stmt: &'a syn::ItemUse);
    fn scan_foreign_fn(&mut self, f: &'a syn::ForeignItemFn);
}

use super::effect::SrcLoc;
use super::hacky_resolver::HackyResolver;
use super::ident::Ident;

pub use super::name_resolution::Resolver;

#[derive(Debug)]
pub struct FileResolver<'a> {
    filepath: &'a FilePath,
    resolver: &'a Resolver,
    backup: HackyResolver<'a>,
}

impl<'a> FileResolver<'a> {
    pub fn new(resolver: &'a Resolver, filepath: &'a FilePath) -> Result<Self> {
        let backup = HackyResolver::new(filepath)?;
        Ok(Self { filepath, resolver, backup })
    }

    fn resolve_core(&self, i: &syn::Ident) -> Result<CanonicalPath> {
        let mut s = SrcLoc::from_span(self.filepath, i);
        // TODO Lydia remove
        s.add1();
        let i = Ident::from_syn(i);
        let result = self
            .resolver
            .resolve_ident(s.clone(), i.clone())
            .with_context(|| format!("{:?} {}", s, i))?;
        Ok(result)
    }

    fn resolve_or_else<F, G, U>(&self, i: &syn::Ident, f: F, g: G) -> U
    where
        F: FnOnce(CanonicalPath) -> U,
        G: FnOnce() -> U,
    {
        match self.resolve_core(i) {
            Ok(res) => f(res),
            Err(err) => {
                eprintln!("Resolution failed for: {}; ", err);
                eprintln!("Falling back to backup resolver");
                g()
            }
        }
    }
}

impl<'a> Resolve<'a> for FileResolver<'a> {
    fn assert_top_level_invariant(&self) {
        self.backup.assert_top_level_invariant();
    }

    fn resolve_ident(&self, i: &'a syn::Ident) -> Path {
        self.resolve_or_else(i, CanonicalPath::to_path, || self.backup.resolve_ident(i))
    }

    fn resolve_path(&self, p: &'a syn::Path) -> Path {
        let i = &p.segments.last().unwrap().ident;
        self.resolve_or_else(i, CanonicalPath::to_path, || self.backup.resolve_path(p))
    }

    fn resolve_def(&self, i: &'a syn::Ident) -> CanonicalPath {
        // eprintln!("resolving def: {:?} ({:?})", i, SrcLoc::from_span(&self.filepath, i));
        self.resolve_or_else(i, |p| p, || self.backup.resolve_def(i))
    }

    fn resolve_ffi(&self, p: &syn::Path) -> Option<CanonicalPath> {
        // TODO: RA implementation
        self.backup.resolve_ffi(p)
    }

    fn push_mod(&mut self, mod_ident: &'a syn::Ident) {
        self.backup.push_mod(mod_ident);
    }

    fn pop_mod(&mut self) {
        self.backup.pop_mod();
    }

    fn push_impl(&mut self, impl_stmt: &'a syn::ItemImpl) {
        self.backup.push_impl(impl_stmt);
    }

    fn pop_impl(&mut self) {
        self.backup.pop_impl();
    }

    fn push_fn(&mut self, fn_ident: &'a syn::Ident) {
        self.backup.push_fn(fn_ident);
    }

    fn pop_fn(&mut self) {
        self.backup.pop_fn();
    }

    fn scan_use(&mut self, use_stmt: &'a syn::ItemUse) {
        self.backup.scan_use(use_stmt);
    }

    fn scan_foreign_fn(&mut self, f: &'a syn::ForeignItemFn) {
        self.backup.scan_foreign_fn(f)
    }
}

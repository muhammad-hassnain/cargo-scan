//! This module defines the core data model for Effects.
//!
//! The main types are:
//! - Effect, which represents an abstract effect
//! - EffectInstance, which represents an instance of an effect in source code
//! - EffectBlock, which represents a block of source code which may contain
//!     zero or more effects (such as an unsafe block).

use super::ident::{CanonicalPath, IdentPath};
use super::sink::Sink;
use super::util::csv;

use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::path::{Path as FilePath, PathBuf as FilePathBuf};
use syn;
use syn::spanned::Spanned;

/*
    Abstraction for identifying a location in the source code --
    essentially derived from syn::Span
*/

/// Data representing a source code location for some identifier, block, or expression
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SrcLoc {
    /// Directory in which the expression occurs
    dir: FilePathBuf,
    /// File in which the expression occurs -- in the above directory
    file: FilePathBuf,
    /// Location in which the expression occurs -- in the above file
    start_line: usize,
    start_col: usize,
    end_line: usize,
    end_col: usize,
}

impl SrcLoc {
    pub fn new(
        filepath: &FilePath,
        start_line: usize,
        start_col: usize,
        end_line: usize,
        end_col: usize,
    ) -> Self {
        // TBD: use unwrap_or_else
        let dir = filepath.parent().unwrap().to_owned();
        let file = FilePathBuf::from(filepath.file_name().unwrap());
        Self { dir, file, start_line, start_col, end_line, end_col }
    }

    pub fn from_span<S>(filepath: &FilePath, span: &S) -> Self
    where
        S: Spanned,
    {
        let span_start = span.span().start();
        let span_end = span.span().end();

        let start_line = span_start.line;
        let start_col = span_start.column;
        let end_line = span_end.line;
        let end_col = span_end.column;

        Self::new(filepath, start_line, start_col, end_line, end_col)
    }

    pub fn sub1(&self) -> Self {
        let mut res = self.clone();
        res.start_line -= 1;
        res.end_line -= 1;
        res
    }

    pub fn add1(&mut self) {
        self.start_col += 1;
    }

    pub fn csv_header() -> &'static str {
        "dir, file, line, col"
    }

    pub fn to_csv(&self) -> String {
        let dir = csv::sanitize_path(&self.dir);
        let file = csv::sanitize_path(&self.file);
        format!("{}, {}, {}, {}", dir, file, self.start_line, self.start_col)
    }

    pub fn dir(&self) -> &FilePathBuf {
        &self.dir
    }

    pub fn file(&self) -> &FilePathBuf {
        &self.file
    }

    pub fn start_line(&self) -> usize {
        self.start_line
    }

    pub fn start_col(&self) -> usize {
        self.start_col
    }

    pub fn end_line(&self) -> usize {
        self.end_line
    }

    pub fn end_col(&self) -> usize {
        self.end_col
    }

    pub fn filepath_string(&self) -> String {
        self.dir.join(&self.file).to_string_lossy().to_string()
    }
}

impl fmt::Display for SrcLoc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}..{}:{}",
            self.filepath_string(),
            self.start_line,
            self.start_col,
            self.end_line,
            self.end_col
        )
    }
}

/*
    Data model for effects
*/

/// Type representing a single effect.
/// For us, this can be any function call to some dangerous function:
/// - a sink pattern in the standard library
/// - an FFI call
/// - an unsafe operation such as a pointer dereference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Function call (callee path) matching a sink pattern
    SinkCall(Sink),
    /// FFI call
    FFICall(CanonicalPath),
    /// Unsafe function/method call
    UnsafeCall(CanonicalPath),
    /// Pointer dereference
    RawPointer(CanonicalPath),
    /// Reading a union field
    UnionField(CanonicalPath),
    /// Accessing a global mutable variable
    StaticMut(CanonicalPath),
    /// Accessing an external mutable variable
    StaticExt(CanonicalPath),
    /// Creation of function pointer
    FnPtrCreation,
    /// Closure creation
    ClosureCreation,
}
impl Effect {
    fn sink_pattern(&self) -> Option<&Sink> {
        match self {
            Self::SinkCall(s) => Some(s),
            Self::FFICall(_) => None,
            Self::UnsafeCall(_) => None,
            Self::RawPointer(_) => None,
            Self::UnionField(_) => None,
            Self::StaticMut(_) => None,
            Self::StaticExt(_) => None,
            Self::FnPtrCreation => None,
            Self::ClosureCreation => None,
        }
    }

    fn simple_str(&self) -> &str {
        match self {
            Self::SinkCall(s) => s.as_str(),
            Self::FFICall(_) => "[FFI]",
            Self::UnsafeCall(_) => "[UnsafeCall]",
            Self::RawPointer(_) => "[PtrDeref]",
            Self::UnionField(_) => "[UnionField]",
            Self::StaticMut(_) => "[StaticMutVar]",
            Self::StaticExt(_) => "[StaticExtVar]",
            Self::FnPtrCreation => "[FnPtrCreation]",
            Self::ClosureCreation => "[ClosureCreation]",
        }
    }

    fn to_csv(&self) -> String {
        csv::sanitize_comma(self.simple_str())
    }
}

/// Type representing an Effect instance, with complete context.
/// This includes a field for which Effect it is an instance of.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct EffectInstance {
    /// Path to the caller function or module scope (Rust path::to::fun)
    caller: CanonicalPath,

    /// Location of call or other effect (Directory, file, line)
    call_loc: SrcLoc,

    /// Callee (effect) function, e.g. libc::sched_getaffinity
    callee: CanonicalPath,

    /// EffectInstance type
    /// If Sink, this includes the effect pattern -- prefix of callee (effect), e.g. libc.
    eff_type: Effect,
}

impl EffectInstance {
    /// Returns a new EffectInstance if the call matches a Sink, is an ffi call,
    /// or is an unsafe call. Regular calls are handled by the explicit call
    /// graph structure.
    pub fn new_call<S>(
        filepath: &FilePath,
        caller: CanonicalPath,
        callee: CanonicalPath,
        callsite: &S,
        is_unsafe: bool,
        ffi: Option<CanonicalPath>,
        sinks: &HashSet<IdentPath>,
    ) -> Option<Self>
    where
        S: Spanned,
    {
        // Code to classify an effect based on call site information
        let call_loc = SrcLoc::from_span(filepath, callsite);
        let eff_type = if let Some(pat) = Sink::new_match(&callee, sinks) {
            if ffi.is_some() {
                // This case occurs for some libc calls
                debug!(
                    "Found FFI callsite also matching a sink pattern; \
                    classifying as SinkCall: \
                    {} ({}) (FFI {:?})",
                    callee, call_loc, ffi
                );
            }
            Some(Effect::SinkCall(pat))
        } else if let Some(ffi) = ffi {
            if !is_unsafe {
                // This case can occur in certain contexts, e.g. with
                // the wasm_bindgen attribute
                debug!(
                    "Found FFI callsite that wasn't marked unsafe; \
                    classifying as FFICall: \
                    {} ({}) (FFI {:?})",
                    callee, call_loc, ffi
                );
            }
            Some(Effect::FFICall(ffi))
        } else if is_unsafe {
            Some(Effect::UnsafeCall(callee.clone()))
        } else {
            None
        };
        Some(Self { caller, call_loc, callee, eff_type: eff_type? })
    }

    pub fn new_effect<S>(
        filepath: &FilePath,
        caller: CanonicalPath,
        callee: CanonicalPath,
        eff_site: &S,
        eff_type: Effect,
    ) -> Self
    where
        S: Spanned,
    {
        let call_loc = SrcLoc::from_span(filepath, eff_site);
        Self { caller, call_loc, callee, eff_type }
    }

    pub fn caller(&self) -> &CanonicalPath {
        &self.caller
    }

    pub fn caller_path(&self) -> &str {
        self.caller.as_str()
    }

    pub fn callee(&self) -> &CanonicalPath {
        &self.callee
    }

    pub fn callee_path(&self) -> &str {
        self.callee.as_str()
    }

    /// Get the caller and callee as full paths
    pub fn caller_callee(&self) -> (&str, &str) {
        (self.caller_path(), self.callee_path())
    }

    pub fn csv_header() -> &'static str {
        "crate, fn_decl, callee, effect, dir, file, line, col"
    }

    pub fn to_csv(&self) -> String {
        let crt = self.caller.crate_name().to_string();
        let caller = self.caller.to_string();
        let callee = csv::sanitize_comma(self.callee.as_str());
        let effect = self.eff_type.to_csv();
        let call_loc_csv = self.call_loc.to_csv();

        format!("{}, {}, {}, {}, {}", crt, caller, callee, effect, call_loc_csv)
    }

    pub fn eff_type(&self) -> &Effect {
        &self.eff_type
    }

    pub fn pattern(&self) -> Option<&Sink> {
        self.eff_type.sink_pattern()
    }

    pub fn is_ffi(&self) -> bool {
        matches!(self.eff_type, Effect::FFICall(_))
    }

    pub fn is_unsafe_call(&self) -> bool {
        matches!(self.eff_type, Effect::UnsafeCall(_))
    }

    pub fn is_ptr_deref(&self) -> bool {
        matches!(self.eff_type, Effect::RawPointer(_))
    }

    pub fn is_union_field_acc(&self) -> bool {
        matches!(self.eff_type, Effect::UnionField(_))
    }

    pub fn is_mut_static(&self) -> bool {
        matches!(self.eff_type, Effect::StaticMut(_))
    }

    pub fn call_loc(&self) -> &SrcLoc {
        &self.call_loc
    }
}

/*
    Data model for effect blocks (unsafe blocks, functions, and impls)
*/

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Visibility {
    Public,
    Private,
}

impl From<&syn::Visibility> for Visibility {
    fn from(vis: &syn::Visibility) -> Self {
        match vis {
            syn::Visibility::Public(_) => Visibility::Public,
            // NOTE: We don't care about public restrictions, only if a function
            //       is visible to other crates
            _ => Visibility::Private,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct FnDec {
    pub src_loc: SrcLoc,
    pub fn_name: CanonicalPath,
    pub vis: Visibility,
}

impl FnDec {
    pub fn new<S>(
        filepath: &FilePath,
        decl_span: &S,
        fn_name: CanonicalPath,
        vis: &syn::Visibility,
    ) -> Self
    where
        S: Spanned,
    {
        let src_loc = SrcLoc::from_span(filepath, decl_span);
        let vis = vis.into();
        Self { src_loc, fn_name, vis }
    }
}

/// Type representing a *block* of zero or more dangerous effects.
/// The block can be:
/// - an expression enclosed by `unsafe { ... }`
/// - a normal function decl `fn foo(args) { ... }`
/// - an unsafe function decl `unsafe fn foo(args) { ... }`
///
/// It also contains a Vector of effects inside the block.
/// However, note that the vector could be empty --
/// we don't currently enumerate all the "bad"
/// things unsafe code could do as individual effects, such as
/// pointer derefs etc.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct EffectBlock {
    // TODO: Include the span in the EffectBlock
    // TODO: Include the ident of the enclosing function
    src_loc: SrcLoc,
    block_type: BlockType,
    effects: Vec<EffectInstance>,
    containing_fn: FnDec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BlockType {
    UnsafeExpr,
    NormalFn,
    UnsafeFn,
}

impl EffectBlock {
    pub fn new_unsafe_expr<S>(
        filepath: &FilePath,
        block_span: &S,
        containing_fn: FnDec,
    ) -> Self
    where
        S: Spanned,
    {
        let src_loc = SrcLoc::from_span(filepath, block_span);
        let block_type = BlockType::UnsafeExpr;
        let effects = Vec::new();
        Self { src_loc, block_type, effects, containing_fn }
    }

    pub fn from_fn_decl(fn_decl: FnDec) -> Self {
        let src_loc = fn_decl.src_loc.clone();
        let block_type = BlockType::NormalFn;
        let effects = Vec::new();
        Self { src_loc, block_type, effects, containing_fn: fn_decl }
    }

    pub fn from_effect(eff: EffectInstance, containing_fn: FnDec) -> Self {
        let mut result = Self::from_fn_decl(containing_fn);
        result.push_effect(eff);
        result
    }

    pub fn new_fn<S>(
        filepath: &FilePath,
        decl_span: &S,
        fn_name: CanonicalPath,
        vis: &syn::Visibility,
    ) -> Self
    where
        S: Spanned,
    {
        let src_loc = SrcLoc::from_span(filepath, decl_span);
        let block_type = BlockType::NormalFn;
        let effects = Vec::new();
        Self {
            src_loc,
            block_type,
            effects,
            containing_fn: FnDec::new(filepath, decl_span, fn_name, vis),
        }
    }

    pub fn new_unsafe_fn<S>(
        filepath: &FilePath,
        decl_span: &S,
        fn_name: CanonicalPath,
        vis: &syn::Visibility,
    ) -> Self
    where
        S: Spanned,
    {
        let src_loc = SrcLoc::from_span(filepath, decl_span);
        let block_type = BlockType::NormalFn;
        let effects = Vec::new();
        Self {
            src_loc,
            block_type,
            effects,
            containing_fn: FnDec::new(filepath, decl_span, fn_name, vis),
        }
    }

    pub fn src_loc(&self) -> &SrcLoc {
        &self.src_loc
    }

    pub fn block_type(&self) -> &BlockType {
        &self.block_type
    }

    pub fn push_effect(&mut self, effect: EffectInstance) {
        self.effects.push(effect);
    }

    pub fn effects(&self) -> &Vec<EffectInstance> {
        &self.effects
    }

    /// Removes all the elements from self.effects for which `f` returns `false`
    /// and returns them
    pub fn filter_effects<F>(&mut self, f: F) -> Vec<EffectInstance>
    where
        F: FnMut(&EffectInstance) -> bool,
    {
        let effects = std::mem::take(&mut self.effects);
        let (new_effects, removed_effects) = effects.into_iter().partition(f);
        self.effects = new_effects;
        removed_effects
    }

    pub fn containing_fn(&self) -> &FnDec {
        &self.containing_fn
    }
}

/// Trait implementations
/// Since an unsafe trait impl cannot itself have any unsafe code,
/// we do not consider it to be an effect block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TraitImpl {
    src_loc: SrcLoc,
    tr_name: CanonicalPath,
    self_type: Option<CanonicalPath>,
}

impl TraitImpl {
    pub fn new<S>(
        impl_span: &S,
        filepath: &FilePath,
        tr_name: CanonicalPath,
        self_type: Option<CanonicalPath>,
    ) -> Self
    where
        S: Spanned,
    {
        let src_loc = SrcLoc::from_span(filepath, impl_span);
        Self { src_loc, tr_name, self_type }
    }
}

/// Trait declarations
/// Since an unsafe trait declaration cannot itself have any unsafe code,
/// we do not consider it to be an effect block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct TraitDec {
    src_loc: SrcLoc,
    tr_name: CanonicalPath,
}
impl TraitDec {
    pub fn new<S>(trait_span: &S, filepath: &FilePath, tr_name: CanonicalPath) -> Self
    where
        S: Spanned,
    {
        let src_loc = SrcLoc::from_span(filepath, trait_span);
        Self { src_loc, tr_name }
    }
}

/*
    Unit tests
*/

#[test]
fn test_csv_header() {
    assert!(EffectInstance::csv_header().ends_with(SrcLoc::csv_header()));
}

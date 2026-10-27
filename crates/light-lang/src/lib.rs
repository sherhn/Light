//! Languages as data. A module brings a [`LangDef`] (JSON); [`Language`] compiles it and gives the editor what it
//! needs - highlighting, bracket matching, an outline, folding, indentation - without any code of its own.

mod def;
mod engine;
mod problems;
mod set;
mod tags;

pub use def::{IndentDef, LangDef, OutlineDef, RuleDef};
pub use engine::{Continuation, FoldKind, IndentMode, Language, class_named, line_starts};
pub use problems::{Problem, ProblemKind};
pub use set::{LanguageSet, matches_file_name};

#[cfg(test)]
mod tests;

//! Slash commands (package C1), ported from the web client:
//!
//! - [`catalog`]: every command and alias, and which are offered where.
//! - [`args`]: guided entry, the argument being typed and what is wrong with it.
//! - [`complete`]: autocomplete for `/` commands, `@` people and `#` channels,
//!   and Tab completion of nicks.
//! - [`run`]: what a typed command does ([`plan`]) and running it ([`execute`]).
//! - [`ignore`]: `/ignore` and `/unignore` against the ignore list.

pub mod args;
pub mod catalog;
pub mod complete;
pub mod ignore;
pub mod run;

pub use catalog::{ALIASES, COMMANDS, Field, FieldKind, Place, Spec, resolve};
pub use run::{Done, NickError, Plan, Problem, View, execute, ignore_outcome, plan};

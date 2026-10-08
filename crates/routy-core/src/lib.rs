//! Routy core: всё, что не UI. Используется и CLI, и desktop-приложением,
//! поэтому поведение у них не может разъехаться.

pub mod discover;
pub mod error;
pub mod expr;
pub mod parser;
pub mod project;
pub mod runner;
pub mod secrets;
pub mod state;
pub mod template;
pub mod vars;

pub use error::{Error, Result};
pub use parser::{Directive, Header, RequestFile, parse};
pub use project::Project;
pub use runner::{RunOutcome, Runner};
pub use vars::Vars;

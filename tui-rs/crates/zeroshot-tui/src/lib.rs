// Pre-existing clippy lints in backend/terminal modules — fix in a dedicated cleanup PR.
#![allow(clippy::type_complexity)]
#![allow(clippy::field_reassign_with_default)]
#![allow(clippy::question_mark)]
#![allow(clippy::while_let_loop)]
#![allow(clippy::needless_return)]

pub mod app;
pub mod backend;
pub mod commands;
pub mod input;
pub mod protocol;
pub mod screens;
pub mod terminal;
pub mod ui;

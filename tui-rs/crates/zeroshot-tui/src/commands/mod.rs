use crate::app::{Action, CommandAction, CommandRequest, ToastLevel};

mod dispatcher;
mod parser;
mod types;

pub use parser::parse;
pub use types::{CommandError, ParsedCommand, VALID_PROVIDERS};

pub fn dispatch(request: CommandRequest) -> Result<Vec<Action>, CommandError> {
    match request {
        CommandRequest::SubmitRaw { raw, context } => match parser::parse(&raw) {
            Ok(parsed) => Ok(dispatcher::dispatch(parsed, context)),
            Err(err) => Ok(vec![Action::Command(CommandAction::ShowToast {
                level: ToastLevel::Error,
                message: err.to_string(),
            })]),
        },
    }
}

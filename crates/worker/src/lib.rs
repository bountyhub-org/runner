use ctx::{Background, Ctx};
use error_stack::Result;

pub mod error;
pub mod shell;

use error::WorkerError;

pub trait Worker: Clone + Send + Sync {
    fn run(self, ctx: Ctx<Background>) -> Result<(), WorkerError>;
}

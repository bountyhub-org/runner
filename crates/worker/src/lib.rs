use ctx::{Background, Ctx};
use miette::Result;

pub mod shell;

pub trait Worker: Clone + Send + Sync {
    fn run(self, ctx: Ctx<Background>) -> Result<()>;
}

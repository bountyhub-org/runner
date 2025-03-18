use ctx::{Background, Ctx};
use miette::Result;

pub mod shell;

pub trait Worker: Clone + Send + Sync {
    async fn run(self, ctx: Ctx<Background>) -> Result<()>;
}

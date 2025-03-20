use miette::Result;
use tokio_util::sync::CancellationToken;

pub mod shell;

pub trait Worker: Clone + Send + Sync {
    async fn run(self, ct: CancellationToken) -> Result<()>;
}

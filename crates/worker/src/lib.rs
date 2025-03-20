use miette::Result;
use tokio_util::sync::CancellationToken;

pub mod shell;

pub trait Worker: Clone + Send + Sync {
    fn run(self, ct: CancellationToken) -> impl std::future::Future<Output = Result<()>> + Send;
}

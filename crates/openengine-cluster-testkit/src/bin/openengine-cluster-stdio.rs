use anyhow::Context;
use openengine_cluster_server::{stdio::serve_ndjson, ConnectionContext, Dispatcher};
use openengine_cluster_testkit::EmptyBackend;
use tokio::io::BufReader;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dispatcher = Dispatcher::new(EmptyBackend, ConnectionContext::new("stdio"));
    serve_ndjson(
        &dispatcher,
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
        tokio::io::stderr(),
    )
    .await
    .context("cluster protocol stdio server failed")
}
